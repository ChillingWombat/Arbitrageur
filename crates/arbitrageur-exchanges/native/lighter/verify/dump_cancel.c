// dump_cancel.c — provenance probe for the vendored Lighter signer.
//
// dlopen()s a signer .so, creates a client with a fixed throwaway test
// key, signs a fixed cancel-order transaction with a fixed nonce, and
// writes the resulting txInfo JSON to a file for verification by
// verifytool (which recomputes the tx hash and runs schnorr.Validate
// against the pinned upstream source).
//
// Usage: dump_cancel <lib.so> <out.json>
//
// The test key is a fixed public test fixture — it controls no funds and
// exists only to exercise the deterministic signing path locally (nonce
// is supplied explicitly, so no network access is needed).
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    uint8_t txType;
    char *txInfo;
    char *txHash;
    char *messageToSign;
    char *err;
} SignedTxResponse;

typedef char *(*CreateClientFn)(char *, char *, int, int, long long);
typedef SignedTxResponse (*SignCancelOrderFn)(
    int, long long, uint8_t, long long, int, long long);

static const char *TEST_KEY =
    "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <lib.so> <out.json>\n", argv[0]);
        return 2;
    }
    void *h = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!h) {
        fprintf(stderr, "dlopen: %s\n", dlerror());
        return 2;
    }
    CreateClientFn cc = (CreateClientFn)dlsym(h, "CreateClient");
    SignCancelOrderFn sc = (SignCancelOrderFn)dlsym(h, "SignCancelOrder");
    if (!cc || !sc) {
        fprintf(stderr, "dlsym failed\n");
        return 2;
    }
    char *err = cc((char *)"https://mainnet.zklighter.elliot.ai",
                   (char *)TEST_KEY, 304, 2, 11111);
    if (err && strlen(err)) {
        fprintf(stderr, "CreateClient: %s\n", err);
        return 1;
    }
    // marketIndex=1 orderIndex=7 skipNonce=1 nonce=43 apiKeyIndex=2
    // accountIndex=11111 — fixed, pure-local signing inputs.
    SignedTxResponse r = sc(1, 7, 1, 43, 2, 11111);
    if (r.err && strlen(r.err)) {
        fprintf(stderr, "SignCancelOrder: %s\n", r.err);
        return 1;
    }
    FILE *f = fopen(argv[2], "w");
    if (!f) {
        perror("fopen");
        return 2;
    }
    fputs(r.txInfo, f);
    fclose(f);
    return 0;
}
