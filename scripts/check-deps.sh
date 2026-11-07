#!/usr/bin/env bash
# Invariant guard (SPEC D6, jifar skill): recording must never depend on the
# network. jifar-audio may not pull in the queue, providers or any HTTP stack.
set -euo pipefail

forbidden='^(jifar-providers|jifar-queue|reqwest|hyper|ureq|isahc|surf) '
deps=$(cargo tree -p jifar-audio -e normal,build --prefix none --format '{p}' | sort -u)

if bad=$(grep -E "$forbidden" <<<"$deps"); then
  echo "error: jifar-audio depends on network/queue crates:" >&2
  echo "$bad" >&2
  exit 1
fi
echo "ok: jifar-audio has no network dependencies"
