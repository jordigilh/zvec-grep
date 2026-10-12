#!/usr/bin/env bash
set -Eeuo pipefail

: "${BUILDARCH:?BUILDARCH is required}"
: "${TARGETARCH:?TARGETARCH is required}"
: "${ZIG_VERSION:?ZIG_VERSION is required}"

case "${BUILDARCH}" in
    amd64)
        zig_arch=x86_64
        zig_sha256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
        ;;
    arm64)
        zig_arch=aarch64
        zig_sha256=958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f
        ;;
    *)
        echo "unsupported build architecture: ${BUILDARCH}" >&2
        exit 1
        ;;
esac

case "${TARGETARCH}" in
    amd64)
        rust_target=x86_64-unknown-linux-gnu
        zig_target=x86_64-linux-gnu
        zig_libc_arch=x86
        ;;
    arm64)
        rust_target=aarch64-unknown-linux-gnu
        zig_target=aarch64-linux-gnu
        zig_libc_arch=aarch64
        ;;
    *)
        echo "unsupported target architecture: ${TARGETARCH}" >&2
        exit 1
        ;;
esac

# Link against the target UBI shared libraries explicitly. UBI glibc contains
# loader-private references that are resolved by the target loader at runtime.
zig_archive="zig-${zig_arch}-linux-${ZIG_VERSION}.tar.xz"
curl --fail --silent --show-error --location \
    "https://ziglang.org/download/${ZIG_VERSION}/${zig_archive}" \
    --output "/tmp/${zig_archive}"
echo "${zig_sha256}  /tmp/${zig_archive}" | sha256sum --check --status
install --directory /opt/zig
tar --extract --file "/tmp/${zig_archive}" --strip-components=1 --directory /opt/zig
rm "/tmp/${zig_archive}"

cat > "/usr/local/bin/${rust_target}-zig-cc" <<EOF
#!/usr/bin/env bash
set -euo pipefail
args=()
link=1
for arg in "\$@"; do
    case "\$arg" in
        --target=${rust_target}|-march=armv8-a|-Wl,--fix-cortex-a53-843419) ;;
        -c|-E|-S|--preprocess) link=0; args+=("\$arg") ;;
        *) args+=("\$arg") ;;
    esac
done
if [[ "\$link" == 1 ]]; then
    args+=(
        /opt/target-libs/libstdc++.so.6
        /opt/target-libs/libc.so.6
        -Wl,--allow-shlib-undefined
    )
fi
exec /opt/zig/zig cc -target ${zig_target} "\${args[@]}"
EOF

cat > "/usr/local/bin/${rust_target}-zig-cxx" <<EOF
#!/usr/bin/env bash
set -euo pipefail
args=()
link=1
for arg in "\$@"; do
    case "\$arg" in
        --target=${rust_target}|-march=armv8-a|-Wl,--fix-cortex-a53-843419) ;;
        -c|-E|-S|--preprocess) link=0; args+=("\$arg") ;;
        *) args+=("\$arg") ;;
    esac
done
if [[ "\$link" == 1 ]]; then
    args+=(
        /opt/target-libs/libstdc++.so.6
        /opt/target-libs/libc.so.6
        -Wl,--allow-shlib-undefined
    )
fi
exec /opt/zig/zig c++ -target ${zig_target} "\${args[@]}"
EOF

cat > "/usr/local/bin/${rust_target}-zig-ar" <<EOF
#!/usr/bin/env bash
exec /opt/zig/zig ar "\$@"
EOF

cat > "/usr/local/bin/${rust_target}-zig-ranlib" <<EOF
#!/usr/bin/env bash
exec /opt/zig/zig ranlib "\$@"
EOF

chmod +x \
    "/usr/local/bin/${rust_target}-zig-cc" \
    "/usr/local/bin/${rust_target}-zig-cxx" \
    "/usr/local/bin/${rust_target}-zig-ar" \
    "/usr/local/bin/${rust_target}-zig-ranlib"

target_env_lower="${rust_target//-/_}"
target_env_upper="${target_env_lower^^}"
bindgen_args="--target=${zig_target} \
    -isystem /opt/zig/lib/include \
    -isystem /opt/zig/lib/libc/include/${zig_libc_arch}-linux-gnu \
    -isystem /opt/zig/lib/libc/include/generic-glibc \
    -isystem /opt/zig/lib/libc/include/${zig_libc_arch}-linux-any \
    -isystem /opt/zig/lib/libc/include/any-linux-any"

export "CARGO_TARGET_${target_env_upper}_LINKER=/usr/local/bin/${rust_target}-zig-cc"
export "CC_${target_env_lower}=/usr/local/bin/${rust_target}-zig-cc"
export "CXX_${target_env_lower}=/usr/local/bin/${rust_target}-zig-cxx"
export "AR_${target_env_lower}=/usr/local/bin/${rust_target}-zig-ar"
export "RANLIB_${target_env_lower}=/usr/local/bin/${rust_target}-zig-ranlib"
export "BINDGEN_EXTRA_CLANG_ARGS_${target_env_lower}=${bindgen_args}"
export BINDGEN_EXTRA_CLANG_ARGS="${bindgen_args}"
export PKG_CONFIG_ALLOW_CROSS=1
export CARGO_TARGET_DIR=/src/target

rustup target add "${rust_target}"
cargo build --locked --release --target "${rust_target}" --package zg

output_dir="${CARGO_TARGET_DIR}/${rust_target}/release"
install --directory /out/bin /out/lib/data/jieba_dict /out/home /out/workspace
install --mode=0755 "${output_dir}/zg" /out/bin/zg
install --mode=0755 "${output_dir}/libzvec_c_api.so" /out/lib/libzvec_c_api.so
install --mode=0644 \
    "${output_dir}/data/jieba_dict/jieba.dict.utf8" \
    /out/lib/data/jieba_dict/jieba.dict.utf8
install --mode=0644 \
    "${output_dir}/data/jieba_dict/hmm_model.utf8" \
    /out/lib/data/jieba_dict/hmm_model.utf8
chown 10001:10001 /out/home /out/workspace
