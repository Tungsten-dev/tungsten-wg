#!/usr/bin/env bash
# Verifies tungsten-core with Verus (see crates/tungsten-core/README.md).
#
#   scripts/verify.sh [cargo args] [-- verus args]
#
# Downloads the pinned Verus release into target/verus/<tag>/ on first use (checksum-checked),
# installs the Rust toolchain it needs with rustup, checks that the vstd pin matches, rejects
# trusted gaps in tungsten-core, then runs `cargo verus verify -p tungsten-core`. A pass proves the
# theorems listed in crates/tungsten-core/VERUS.md.
#
# To bump Verus: change VERUS_TAG, VSTD_VERSION, VERUS_TOOLCHAIN and the checksums together, and
# the `vstd` pin in the root Cargo.toml. The pin check enforces the last part. VSTD_VERSION and
# VERUS_TOOLCHAIN are in Verus's own manifest for the release
# (source/cargo-verus/toolchain-manifests/<tag>.toml) and in version.json in the release zip.

set -euo pipefail

VERUS_TAG="0.2026.10.26.d17fbd0"
VSTD_VERSION="0.0.0-2026-10-25-0158"
VERUS_TOOLCHAIN_VERSION="1.99.1"

# Per platform: asset suffix, sha256, rustup host triple.
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)
        PLATFORM="x86-linux"
        SHA256="361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940"
        TRIPLE="x86_64-unknown-linux-gnu"
        ;;
    Linux-aarch64 | Linux-arm64)
        PLATFORM="arm64-linux"
        SHA256="55d0179000c2c78732825d47a8b8fed1aa4afbe6d24896acc6e060eddcd338e4"
        TRIPLE="aarch64-unknown-linux-gnu"
        ;;
    Darwin-x86_64)
        PLATFORM="x86-macos"
        SHA256="24a23b0267f7a225c6ece4bc019930ca273bd57207fe0c1dd3cc7dc5726a37bc"
        TRIPLE="x86_64-apple-darwin"
        ;;
    Darwin-arm64)
        PLATFORM="arm64-macos"
        SHA256="5e62be446a5da782d64caeee41bb1490c8aa3c65c0afe380d1c1dcbe207571ff"
        TRIPLE="aarch64-apple-darwin"
        ;;
    *)
        echo "verify.sh: unsupported platform $(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ASSET="verus-${VERUS_TAG}-${PLATFORM}.zip"
URL="https://github.com/verus-lang/verus/releases/download/release/${VERUS_TAG}/${ASSET}"
CACHE="$ROOT/target/verus/$VERUS_TAG"
VERUS_DIR="$CACHE/verus-$PLATFORM"
TOOLCHAIN="${VERUS_TOOLCHAIN_VERSION}-${TRIPLE}"

sha256() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# 1. Pin check: the vstd pin in Cargo.toml must be the one this Verus release was published with.
PINNED="$(sed -n 's/^vstd *= *"=\([^"]*\)".*/\1/p' Cargo.toml)"
if [[ "$PINNED" != "$VSTD_VERSION" ]]; then
    echo "verify.sh: Cargo.toml pins vstd \"=$PINNED\", but Verus $VERUS_TAG needs \"=$VSTD_VERSION\"." >&2
    echo "Bump VERUS_TAG, VSTD_VERSION, the checksums and the vstd pin together." >&2
    exit 1
fi

# 2. Trusted-gap guard. Nothing may be trusted without proof, except the float primitives in
#    prim_float.rs: exactly 22 external_body functions, each equated to one of exactly 22
#    uninterpreted functions in spec/prim.rs.
SRC=crates/tungsten-core/src
if GAPS="$(grep -rnE 'admit\(|assume\(|external_body|verifier::external|assume_specification|axiom fn|exec_allows_no_decreases_clause|assume_termination' "$SRC" --include='*.rs' | grep -v "^$SRC/prim_float\.rs:")"; then
    echo "verify.sh: trusted gaps in tungsten-core:" >&2
    echo "$GAPS" >&2
    exit 1
fi
if GAPS="$(grep -rnE '\buninterp\b' "$SRC" --include='*.rs' | grep -vE "^$SRC/(spec/prim|prim_float)\.rs:")"; then
    echo "verify.sh: uninterpreted functions outside spec/prim.rs:" >&2
    echo "$GAPS" >&2
    exit 1
fi
N_BODIES="$(grep -cE '^#\[verifier::external_body\]' "$SRC/prim_float.rs" || true)"
N_UNINTERP="$(grep -cE '^pub uninterp spec fn' "$SRC/spec/prim.rs" || true)"
if [[ "$N_BODIES" != 22 || "$N_UNINTERP" != 22 ]]; then
    echo "verify.sh: expected 22 trusted primitives, found $N_BODIES bodies and $N_UNINTERP uninterpreted functions." >&2
    echo "Changing the trusted base is a decision: update this check, and SPEC.md's list, on purpose." >&2
    exit 1
fi

# 3. Download and unpack the release once. Nothing below needs the network after that, except
#    rustup the first time.
if [[ ! -x "$VERUS_DIR/cargo-verus" ]]; then
    echo "verify.sh: downloading Verus $VERUS_TAG ($PLATFORM)"
    TMP="$(mktemp -d "$ROOT/target/verus-download.XXXXXX")"
    trap 'rm -rf "$TMP"' EXIT
    curl -fsSL -o "$TMP/$ASSET" "$URL"
    GOT="$(sha256 "$TMP/$ASSET")"
    if [[ "$GOT" != "$SHA256" ]]; then
        echo "verify.sh: checksum mismatch for $ASSET: expected $SHA256, got $GOT" >&2
        exit 1
    fi
    mkdir -p "$TMP/unpacked"
    unzip -q "$TMP/$ASSET" -d "$TMP/unpacked"
    if [[ ! -x "$TMP/unpacked/verus-$PLATFORM/cargo-verus" ]]; then
        echo "verify.sh: $ASSET has no verus-$PLATFORM/cargo-verus" >&2
        exit 1
    fi
    mkdir -p "$CACHE"
    rm -rf "$VERUS_DIR"
    mv "$TMP/unpacked/verus-$PLATFORM" "$VERUS_DIR"
    rm -rf "$TMP"
    trap - EXIT
fi

# 4. The Rust toolchain this Verus release is built against.
if ! rustup run "$TOOLCHAIN" rustc --version >/dev/null 2>&1; then
    rustup toolchain install --profile minimal "$TOOLCHAIN"
fi

# 5. The release's z3 is built against a recent glibc (2.38 on x86-linux). If it doesn't start
#    here, point VERUS_Z3_PATH at a Z3 4.16.0 binary built for this machine.
if [[ -z "${VERUS_Z3_PATH:-}" ]] && ! "$VERUS_DIR/z3" --version >/dev/null 2>&1; then
    echo "verify.sh: $VERUS_DIR/z3 doesn't run on this machine." >&2
    echo "Set VERUS_Z3_PATH to a Z3 4.16.0 binary, e.g. VERUS_Z3_PATH=/path/to/z3 scripts/verify.sh" >&2
    exit 1
fi

# 6. Verify. Arguments pass through (`-- <verus args>` goes to Verus).
export PATH="$VERUS_DIR:$PATH"
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
exec cargo verus verify -p tungsten-core "$@"
