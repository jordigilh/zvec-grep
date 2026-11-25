#!/usr/bin/env python3
"""MLX GPU ceiling spike for zvec-grep issue #74 (see README.md in this dir).

Implements the JinaBERT forward pass (ALiBi + qk-LayerNorm + GLU FFN,
post-norm residuals) in MLX from the fp16 safetensors weights, validates
numerics against the ONNX artifacts, and measures throughput against ORT CPU
q8 on identical batches.

Prerequisites (CLT-only host is fine; the Python wheels ship prebuilt MLX):

    python3 -m venv .venv && .venv/bin/pip install mlx numpy safetensors \
        tokenizers onnxruntime huggingface_hub

Usage:

    .venv/bin/python jina_mlx_spike.py --corpus <repo>/rust/crates/zg-engine/src

The resolved jina-embeddings-v2-base-code artifacts are located in zg's
model cache (`$ZVEC_GREP_MODEL_CACHE`, else `$ZVEC_GREP_HOME/models`, else
`~/.zvec-grep/models`, revision 516f4ba...); anything missing is fetched
from the same pinned revision. The fp16 safetensors and fp32 ONNX reference
are always fetched on first run. Use `--dtype bf16` for the bf16 variant,
`--eager` to skip `mx.compile`, and `--skip-checks` to run the MLX benchmark
alone in a fresh process (this is how the MLX-only peak RSS is measured,
without the in-process ONNX reference sessions).
"""
import argparse
import glob
import os
import resource
import sys
import time

import mlx.core as mx
import numpy as np
from safetensors import safe_open
from tokenizers import Tokenizer

REPO = "jinaai/jina-embeddings-v2-base-code"
REVISION = "516f4baf13dec4ddddda8631e019b5737c8bc250"
H, L, HEADS = 768, 12, 12
EPS = 1e-12
# Recovered from the shipped ONNX graph (see README).
SLOPES = mx.array([-0.5, -0.25, -0.125, -0.0625, -0.03125, -0.015625,
                   -0.0078125, -0.00390625,
                   -0.7071067690849304, -0.3535533845424652,
                   -0.1767766922712326, -0.0883883461356163],
                  dtype=mx.float32).reshape(1, HEADS, 1, 1)


def fetch(name):
    """Fetch a file from the pinned revision (cached by huggingface_hub)."""
    from huggingface_hub import hf_hub_download
    return hf_hub_download(REPO, name, revision=REVISION)


def load_weights():
    path = fetch("model.safetensors")
    weights = {}
    with safe_open(path, framework="numpy") as f:
        for k in f.keys():
            weights[k] = mx.array(f.get_tensor(k))
    return weights


def layer_norm(x, gain, bias):
    mu = mx.mean(x, axis=-1, keepdims=True)
    d = x - mu
    var = mx.mean(d * d, axis=-1, keepdims=True)
    return d / mx.sqrt(var + EPS) * gain + bias


def alibi_bias(n):
    pos = mx.arange(n, dtype=mx.float32)
    dist = mx.abs(pos.reshape(1, n) - pos.reshape(n, 1)).reshape(1, 1, n, n)
    return SLOPES * dist


def forward(w, ids, mask):
    b, n = ids.shape
    x = w["embeddings.word_embeddings.weight"][ids] + w["embeddings.token_type_embeddings.weight"][0]
    x = layer_norm(x, w["embeddings.LayerNorm.weight"], w["embeddings.LayerNorm.bias"])
    alibi = alibi_bias(n)
    mask_bias = (1 - mask.astype(mx.float32)).reshape(b, 1, 1, n) * -3.4028235e38
    for i in range(L):
        p = f"encoder.layer.{i}."
        q = x @ w[p + "attention.self.query.weight"].T + w[p + "attention.self.query.bias"]
        k = x @ w[p + "attention.self.key.weight"].T + w[p + "attention.self.key.bias"]
        v = x @ w[p + "attention.self.value.weight"].T + w[p + "attention.self.value.bias"]
        q = layer_norm(q, w[p + "attention.self.layer_norm_q.weight"], w[p + "attention.self.layer_norm_q.bias"])
        k = layer_norm(k, w[p + "attention.self.layer_norm_k.weight"], w[p + "attention.self.layer_norm_k.bias"])
        q = q.reshape(b, n, HEADS, H // HEADS).transpose(0, 2, 1, 3)
        k = k.reshape(b, n, HEADS, H // HEADS).transpose(0, 2, 1, 3)
        v = v.reshape(b, n, HEADS, H // HEADS).transpose(0, 2, 1, 3)
        scores = (q @ k.transpose(0, 1, 3, 2)) / 8.0 + alibi + mask_bias
        a = mx.softmax(scores, axis=-1) @ v
        a = a.transpose(0, 2, 1, 3).reshape(b, n, H)
        ao = a @ w[p + "attention.output.dense.weight"].T + w[p + "attention.output.dense.bias"]
        a_ln = layer_norm(ao + x, w[p + "attention.output.LayerNorm.weight"], w[p + "attention.output.LayerNorm.bias"])
        x1 = x + a_ln
        h = layer_norm(x1, w[p + "layer_norm_1.weight"], w[p + "layer_norm_1.bias"])
        ug = h @ w[p + "mlp.up_gated_layer.weight"].T
        gate, up = ug[..., :3072], ug[..., 3072:]
        gelu = up * (1.0 + mx.erf(up / float(np.sqrt(2.0)))) * 0.5
        down = (gate * gelu) @ w[p + "mlp.down_layer.weight"].T + w[p + "mlp.down_layer.bias"]
        x = layer_norm(h + down, w[p + "layer_norm_2.weight"], w[p + "layer_norm_2.bias"])
    return x


def pooled(w, ids, mask):
    x = forward(w, ids, mask)
    m = mask.astype(mx.float32)
    p = (x * m[..., None]).sum(axis=1) / m.sum(axis=1, keepdims=True)
    p = p.astype(mx.float32)
    return p / mx.sqrt((p * p).sum(axis=1, keepdims=True))


def corpus_texts(corpus):
    texts = []
    for path in sorted(glob.glob(os.path.join(corpus, "**", "*.rs"), recursive=True)):
        with open(path, encoding="utf-8", errors="ignore") as f:
            texts.append(f.read()[:6000])
    return texts


def numerics_check(w, tokenizer, onnx_path):
    """Return (cosine vs fp32 ONNX, cosine vs cached q8 ONNX) per text."""
    import onnxruntime as ort
    texts = ["def main():\n    print('hello world')\n    return 0\n",
             "pub fn embed_batch(loaded: &LoadedModel) -> Result<()> { run_cpu_batched() }",
             "SELECT * FROM users WHERE id = 42; -- fetch user row"]
    enc = tokenizer.encode_batch(texts)
    ids = mx.array([e.ids for e in enc], dtype=mx.uint32)
    mask = mx.array([e.attention_mask for e in enc], dtype=mx.float32)
    ours = np.array(pooled(w, ids, mask))
    tm = np.array([e.attention_mask for e in enc], dtype=np.float32)

    def session_cosines(path):
        session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
        out = session.run(None, {
            "input_ids": np.array([e.ids for e in enc], dtype=np.int64),
            "attention_mask": np.array([e.attention_mask for e in enc], dtype=np.int64),
        })[0].astype(np.float32)
        ref = (out * tm[..., None]).sum(1) / tm.sum(1, keepdims=True)
        ref = ref / np.linalg.norm(ref, axis=1, keepdims=True)
        return (ours * ref).sum(1)

    cos_fp32 = session_cosines(onnx_path)
    q8 = default_cache_q8() or fetch("onnx/model_quantized.onnx")
    cos_q8 = session_cosines(q8)
    return cos_fp32, cos_q8


def run_mlx(w, batches, enc, compiled):
    fwd = mx.compile(forward) if compiled else forward
    def step(eb):
        ids = mx.array([x.ids for x in eb], dtype=mx.uint32)
        mask = mx.array([x.attention_mask for x in eb], dtype=mx.float32)
        x = fwd(w, ids, mask)
        m = mask.astype(mx.float32)
        p = (x * m[..., None]).sum(1) / m.sum(1, keepdims=True)
        p = p.astype(mx.float32)
        mx.eval(p / mx.sqrt((p * p).sum(1, keepdims=True)))
    step([enc[i] for i in batches[0]])  # warmup
    t0 = time.perf_counter()
    for batch in batches:
        step([enc[i] for i in batch])
    return time.perf_counter() - t0


def run_ort(onnx_path, batches, enc, threads):
    import onnxruntime as ort
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = threads
    session = ort.InferenceSession(onnx_path, opts, providers=["CPUExecutionProvider"])
    def step(eb):
        session.run(None, {
            "input_ids": np.array([x.ids for x in eb], dtype=np.int64),
            "attention_mask": np.array([x.attention_mask for x in eb], dtype=np.int64),
        })
    step([enc[i] for i in batches[0]])  # warmup
    t0 = time.perf_counter()
    for batch in batches:
        step([enc[i] for i in batch])
    return time.perf_counter() - t0


def default_cache_q8():
    """Locate the cached q8 artifact in zg's model-cache layout."""
    cache = (os.environ.get("ZVEC_GREP_MODEL_CACHE")
             or os.path.join(os.environ.get("ZVEC_GREP_HOME",
                                            os.path.expanduser("~/.zvec-grep")),
                             "models"))
    for root, _, files in os.walk(cache):
        if "model_quantized.onnx" in files and REVISION in root and "jina" in root:
            return os.path.join(root, "model_quantized.onnx")
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True, help="directory of .rs files to embed")
    ap.add_argument("--batch", type=int, default=8)
    ap.add_argument("--threads", type=int, default=12, help="ORT intra-op threads")
    ap.add_argument("--dtype", choices=["fp16", "bf16"], default="fp16")
    ap.add_argument("--eager", action="store_true", help="skip mx.compile")
    ap.add_argument("--skip-checks", action="store_true",
                    help="skip the numerics check and the ORT benchmark "
                         "(MLX-only run: measures MLX-only peak RSS)")
    args = ap.parse_args()

    w = load_weights()
    if args.dtype == "bf16":
        w = {k: (v.astype(mx.bfloat16) if v.dtype in (mx.float16, mx.float32) else v)
             for k, v in w.items()}
    tokenizer = Tokenizer.from_file(fetch("tokenizer.json"))
    tokenizer.enable_padding(length=1024)
    tokenizer.enable_truncation(max_length=1024)

    if not args.skip_checks:
        fp32_onnx = fetch("onnx/model.onnx")
        cos_fp32, cos_q8 = numerics_check(w, tokenizer, fp32_onnx)
        print("numerics cosine(mlx, onnx fp32):", np.round(cos_fp32, 7).tolist())
        print("numerics cosine(mlx, onnx q8):", np.round(cos_q8, 4).tolist())
        if not all(c >= 0.999 for c in cos_fp32):
            sys.exit("numerics check failed")

    texts = corpus_texts(args.corpus)
    enc = tokenizer.encode_batch(texts)
    order = sorted(range(len(enc)), key=lambda i: -sum(enc[i].attention_mask))
    batches = [order[i:i + args.batch] for i in range(0, len(order), args.batch)]
    total_tokens = sum(sum(enc[i].attention_mask) for i in order)
    print(f"corpus: {len(texts)} files, {total_tokens} real tokens, "
          f"{len(batches)} batches of {args.batch}, dtype {args.dtype}, "
          f"{'eager' if args.eager else 'compiled'}")

    dt = run_mlx(w, batches, enc, compiled=not args.eager)
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1e9
    print(f"MLX GPU {args.dtype} {'eager' if args.eager else 'compiled'}: "
          f"{dt:.1f}s, {total_tokens/dt:.0f} tok/s, "
          f"{len(texts)/dt:.2f} files/s, peak RSS {rss:.2f} GB"
          f"{' (MLX-only)' if args.skip_checks else ' (includes ONNX sessions)'}")

    if args.skip_checks:
        return
    cache_q8 = default_cache_q8()
    if cache_q8 is None:
        cache_q8 = fetch("onnx/model_quantized.onnx")
    dt = run_ort(cache_q8, batches, enc, args.threads)
    print(f"ORT CPU q8 ({args.threads} threads): {dt:.1f}s, {total_tokens/dt:.0f} tok/s, "
          f"{len(texts)/dt:.2f} files/s")


if __name__ == "__main__":
    main()
