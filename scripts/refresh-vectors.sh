#!/usr/bin/env bash
# Refresh tests/vectors from the Go commit named in proto/GO_REF.
# --check re-downloads that commit and fails when the files would change.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CHECK=0
if [ "${1:-}" = "--check" ]; then
  CHECK=1
elif [ -n "${1:-}" ]; then
  echo "usage: scripts/refresh-vectors.sh [--check]" >&2
  exit 2
fi

OUT="${ROOT}/tests/vectors"
if [ "${CHECK}" -eq 1 ]; then
  OUT="$(mktemp -d)"
  trap 'rm -rf "${OUT}"' EXIT
fi

python3 - "${ROOT}" "${OUT}" <<'PY'
import pathlib, re, sys, urllib.request

root = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

go_ref = (root / "proto/GO_REF").read_text()

def field(name):
    match = re.search(rf"^{name}=(\S+)", go_ref, re.M)
    if not match:
        sys.exit(f"proto/GO_REF has no {name}")
    return match.group(1)

commit = field("COMMIT")
tag = field("GO_REF")
repo = field("REPO")
# https://github.com/eldnetwork/eld-tendermint -> raw host path
repo_path = repo.removeprefix("https://github.com/")
raw = f"https://raw.githubusercontent.com/{repo_path}/{commit}"

def download(path):
    url = f"{raw}/{path}"
    try:
        with urllib.request.urlopen(url, timeout=60) as resp:
            return resp.read().decode()
    except Exception as err:
        sys.exit(f"failed to download {url}: {err}")

def function_body(text, name):
    match = re.search(rf"^func {name}\b", text, re.M)
    if not match:
        sys.exit(f"missing func {name}")
    rest = text[match.end():]
    nxt = re.search(r"^func ", rest, re.M)
    return rest[: nxt.start()] if nxt else rest

def hex_strings(body):
    found = []
    for literal in re.findall(r'"([^"\n]*)"', body):
        if literal and len(literal) % 2 == 0 and re.fullmatch(r"[0-9a-fA-F]+", literal):
            found.append(literal.lower())
    return found

def byte_slices(body):
    """Each []byte{ 0xNN, ... } in source order, comments ignored."""
    slices = []
    for block in re.findall(r"\[\]byte\{([^{}]*)\}", body):
        nums = re.findall(r"0x([0-9a-fA-F]+)", block)
        if nums:
            slices.append("".join(f"{int(n, 16):02x}" for n in nums))
    return slices

def write(name, source, lines):
    header = [
        f"# {source}",
        f"# COMMIT={commit}",
        f"# GO_REF={tag}",
    ]
    text = "\n".join(header + lines) + "\n"
    (out / name).write_text(text)

cases = [
    ("mempool-v0.hex", "mempool/v0/reactor_test.go", "TestMempoolVectors", "hex"),
    ("mempool-v1.hex", "mempool/v1/reactor_test.go", "TestMempoolVectors", "hex"),
    ("blockchain.hex", "blockchain/msgs_test.go", "TestBlockchainMessageVectors", "hex"),
    ("privval.hex", "privval/msgs_test.go", "TestPrivvalVectors", "hex"),
    ("consensus.hex", "consensus/msgs_test.go", "TestConsMsgsVectors", "hex"),
    ("pex.hex", "p2p/pex/pex_reactor_test.go", "TestPexVectors", "hex"),
    ("conn.hex", "p2p/conn/connection_test.go", "TestConnVectors", "hex"),
    ("statesync.hex", "statesync/messages_test.go", "TestStateSyncVectors", "hex"),
    ("evidence.hex", "evidence/reactor_test.go", "TestEvidenceVectors", "hex"),
    ("vote-sign-bytes.hex", "types/vote_test.go", "TestVoteSignBytesTestVectors", "bytes"),
]

for filename, path, func, kind in cases:
    body = function_body(download(path), func)
    lines = byte_slices(body) if kind == "bytes" else hex_strings(body)
    if not lines:
        sys.exit(f"{path} {func} produced no vectors")
    write(filename, f"{path} {func}", lines)

# These Go tests compare behavior and do not embed hex. The files record that.
abci = function_body(download("abci/types/messages_test.go"), "TestWriteReadMessageSimple")
if "Hello" not in abci:
    sys.exit("TestWriteReadMessageSimple no longer frames RequestEcho Hello")
if hex_strings(abci):
    sys.exit("TestWriteReadMessageSimple now has static hex; add it to the extractor")
write(
    "abci-frame.hex",
    "abci/types/messages_test.go TestWriteReadMessageSimple (no static hex; framed RequestEcho Hello)",
    ["0e0a0548656c6c6f"],
)

results = function_body(download("types/results_test.go"), "TestABCIResults")
if "Data: nil" not in results or "[]byte{}" not in results:
    sys.exit("TestABCIResults no longer compares nil and empty Data")
write(
    "abci-results.txt",
    "types/results_test.go TestABCIResults (no static hex; nil Data matches empty Data)",
    [],
)

golden = download("p2p/conn/testdata/TestDeriveSecretsAndChallengeGolden.golden")
if not golden.endswith("\n"):
    golden += "\n"
(out / "secret-connection.golden").write_text(golden)

(out / "SOURCE").write_text(
    f"GO_REF={tag}\nCOMMIT={commit}\nREPO={repo}\n"
)
print(f"wrote vectors for {commit} into {out}")
PY

if [ "${CHECK}" -eq 1 ]; then
  if ! diff -ru "${ROOT}/tests/vectors" "${OUT}" >/dev/null; then
    echo "tests/vectors does not match proto/GO_REF. Run scripts/refresh-vectors.sh" >&2
    diff -ru "${ROOT}/tests/vectors" "${OUT}" || true
    exit 1
  fi
fi
