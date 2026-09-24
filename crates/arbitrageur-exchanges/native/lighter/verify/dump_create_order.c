// dump_create_order.c — provenance probe for the vendored Lighter signer,
// covering the HOT signing path (order entry).
//
// dlopen()s a signer .so, creates a client with a fixed throwaway test
// key, signs a fixed create-order transaction with a fixed nonce and a
// fixed expiry, and writes the resulting txInfo JSON to a file for
// verification by verifytool (which recomputes the tx hash and runs
// schnorr.Validate against the pinned upstream source).
//
// Usage: dump_create_order <lib.so> <out.json>
//
// The test key is a fixed public test fixture — it controls no funds and
// exists only to exercise the deterministic signing path locally (nonce
// and expiry are supplied explicitly, so no network access is needed).
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
// Mirrors liblighter_signer.h: 19 args in declaration order.
typedef SignedTxResponse (*SignCreateOrderFn)(
    int, long long, long long, int, int, int, int, int, int, long long,
    long long, int, int, uint8_t, uint8_t, uint8_t, long long, int,
    long long);

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
    SignCreateOrderFn sc = (SignCreateOrderFn)dlsym(h, "SignCreateOrder");
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
    // marketIndex=1 clientOrderIndex=99 baseAmount=100000 price=65000
    // isAsk=0 orderType=0(limit) tif=1(GTT) reduceOnly=0 triggerPrice=0
    // orderExpiry=1750000000000 (fixed — keeps the probe deterministic)
    // integratorAccountIndex=0 takerFee=0 makerFee=0
    // selfTradeBehavior=0 selfTradeEquality=0 skipNonce=1 nonce=43
    // apiKeyIndex=2 accountIndex=11111 — fixed, pure-local signing inputs.
    SignedTxResponse r = sc(1, 99, 100000, 65000, 0, 0, 1, 0, 0,
                            1750000000000LL, 0, 0, 0, 0, 0, 1, 43, 2,
                            11111);
    if (r.err && strlen(r.err)) {
        fprintf(stderr, "SignCreateOrder: %s\n", r.err);
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
