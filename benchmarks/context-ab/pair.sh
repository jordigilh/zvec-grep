#!/usr/bin/env zsh
# Build one A/B pair for a context-cap experiment: copy the corpus twice and
# index one side with each binary. The two binaries must differ in the catalog
# max_input_tokens (or chunk budget) under test; everything else should match.
#
# Usage:
#   ZG_A_BIN=<old-binary> ZG_B_BIN=<new-binary> \
#   pair.sh <name> <corpus-src> <model> [rsync-exclude-args...]
#
# Use ANCHORED rsync excludes ("/target", "/node_modules") — an unanchored
# "--exclude analysis" once silently dropped a source package directory named
# "analysis" and shrank the corpus.
set -euo pipefail
NAME=$1
SRC=$2
MODEL=$3
shift 3
A_BIN=${ZG_A_BIN:?set ZG_A_BIN to the baseline (old-cap) binary}
B_BIN=${ZG_B_BIN:?set ZG_B_BIN to the experimental (new-cap) binary}
BASE=${ZG_AB_BASE:-$(mktemp -d)}
MODEL_CACHE=${ZG_GREP_MODEL_CACHE:-$HOME/.engram/zvec-grep/model}
mkdir -p "$BASE"
echo "work base: $BASE"
for side in a b; do
  rm -rf "$BASE/$NAME-$side-work" "$BASE/$NAME-$side-home"
  mkdir -p "$BASE/$NAME-$side-work" "$BASE/$NAME-$side-home"
  rsync -a "$@" --exclude .git --exclude .zvec-grep "$SRC"/ "$BASE/$NAME-$side-work/"
done
for side in a b; do
  BIN=$A_BIN; [ $side = b ] && BIN=$B_BIN
  echo "=== indexing $NAME-$side with $BIN"
  cd "$BASE/$NAME-$side-work"
  START=$(date +%s)
  if ZVEC_GREP_MODE=direct ZVEC_GREP_HOME="$BASE/$NAME-$side-home" \
    "$BIN" --index --embedding $MODEL \
    --model-cache "$MODEL_CACHE" --device cpu --mode direct \
    > "$BASE/$NAME-$side.index.log" 2>&1
  then
    echo "label=$NAME-$side exit=0 wall=$(( $(date +%s) - START ))s"
  else
    status=$?
    echo "label=$NAME-$side exit=$status wall=$(( $(date +%s) - START ))s (see $BASE/$NAME-$side.index.log)"
    exit $status
  fi
done
echo "PAIR_DONE $NAME"
