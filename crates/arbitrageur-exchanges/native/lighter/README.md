# Vendored Lighter signer

`liblighter_signer.so` is a C-shared build of the upstream Go package
`github.com/elliottech/lighter-go/sharedlib`. It is used only for local Lighter
transaction signing; the Rust adapter still performs nonce lookup and HTTP
`sendTx` submission separately.

## Verified binary identity

- File: `liblighter_signer.so`
- SHA-256: `8cb2fbcadab0b9b3fed4a524998b825818b677fcf9c49bc1f18b4ca02a233753`
- Embedded module metadata observed in the binary:
  - `github.com/elliottech/lighter-go` `v1.0.10`
  - Go toolchain `<go1.24.6`
- Header: `liblighter_signer.h`

The Rust loader hashes the file before `dlopen`. A digest mismatch is a hard
error and never falls back to another candidate. CI repeats the same digest
check in `.github/workflows/ci.yml`.

## Pinned upstream source

- Repository: `https://github.com/elliottech/lighter-go`
- Tag: `v1.0.10`
- Commit: `9d38261d1a4cc5c7211b383ba07a4d6e41604708`
- Build recipe (from upstream `justfile`, `build-linux-amd64-docker`):
  `CGO_ENABLED=1 GOOS=linux GOARCH=amd64 go build -buildmode=c-shared
  -trimpath -o lighter-signer-linux-amd64.so ./sharedlib`
- Toolchain: Go 1.24.6 (matches the embedded build metadata)

## Functional provenance verification

`verify/verify_provenance.sh` performs an end-to-end equivalence check:

1. Clones the pinned commit and rebuilds the signer from source.
2. Signs a fixed cancel-order transaction (fixed throwaway test key, fixed
   nonce — pure-local, no network) with both the vendored and rebuilt
   binaries via a small `dlopen` C driver (`verify/dump_cancel.c`).
3. Verifies both signatures with the pinned source's own
   `schnorr.Validate` against the derived public key
   (`verify/verifytool/main.go`), and byte-compares the non-signature
   transaction fields.

A passing run proves the vendored binary implements identical key
derivation, Goldilocks-field arithmetic, Schnorr signing, and transaction
hashing as the pinned commit. Last verified result:

- Derived pubkey (both binaries): `0xdd463e68f02bcf06a4a0d7af954adb68f436c7b409a2baae4d60dff5178f8f615f8f1a756d04d528`
- `schnorr.Validate`: PASS for vendored and rebuilt outputs
- CI runs this check in the `signer-provenance` job.

Honest limitation: Go `c-shared` builds are not bit-reproducible across
environments, so the rebuilt digest legitimately differs from the vendored
digest — equivalence is verified cryptographically at the signing surface
rather than bytewise. Code paths the adapter never calls remain unexercised
by this check.

## Override policy

`LIGHTER_SIGNER_LIB` may point to a custom signer only when
`LIGHTER_SIGNER_SHA256` is also set to that file's exact lowercase or uppercase
64-hex SHA-256. Overrides fail closed: no fallback to the sidecar or vendored
binary is attempted after an override failure.

When upgrading the vendored binary: bump the pin constants in
`verify/verify_provenance.sh`, run it to confirm equivalence, record the new
SHA-256 here and in the Rust loader's approved digest, then review the new
binary before commit.
