// verifytool — signature verifier for the Lighter signer provenance check.
//
// Copied into a pinned-source checkout of elliottech/lighter-go by
// verify_provenance.sh, then run against a txInfo JSON produced by the
// signer binary under test. It recomputes the transaction hash and runs
// schnorr.Validate under the public key derived from the fixed test
// private key — so a passing result proves the binary under test
// implements the same curve, field arithmetic, key derivation, and
// transaction hashing as the pinned source.
//
// Usage: verifytool <txinfo.json> <privkey-hex> <chainId> [cancel|create]
package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/elliottech/lighter-go/signer"
	"github.com/elliottech/lighter-go/types/txtypes"
	schnorr "github.com/elliottech/poseidon_crypto/signature/schnorr"
)

func main() {
	if len(os.Args) != 4 && len(os.Args) != 5 {
		fmt.Fprintln(os.Stderr, "usage: verifytool <txinfo.json> <privkey-hex> <chainId> [cancel|create]")
		os.Exit(2)
	}
	kind := "cancel"
	if len(os.Args) == 5 {
		kind = os.Args[4]
	}
	var chainId uint32
	if _, err := fmt.Sscanf(os.Args[3], "%d", &chainId); err != nil {
		fmt.Fprintln(os.Stderr, "chainId:", err)
		os.Exit(2)
	}

	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, "read:", err)
		os.Exit(2)
	}

	var msgHash []byte
	var sig []byte
	switch kind {
	case "cancel":
		var tx txtypes.L2CancelOrderTxInfo
		if err := json.Unmarshal(data, &tx); err != nil {
			fmt.Fprintln(os.Stderr, "unmarshal:", err)
			os.Exit(2)
		}
		msgHash, err = tx.Hash(chainId)
		sig = tx.Sig
	case "create":
		var tx txtypes.L2CreateOrderTxInfo
		if err := json.Unmarshal(data, &tx); err != nil {
			fmt.Fprintln(os.Stderr, "unmarshal:", err)
			os.Exit(2)
		}
		msgHash, err = tx.Hash(chainId)
		sig = tx.Sig
	default:
		fmt.Fprintln(os.Stderr, "unknown tx kind:", kind)
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "hash:", err)
		os.Exit(2)
	}
	keyBytes, err := hexutil.Decode(os.Args[2])
	if err != nil {
		fmt.Fprintln(os.Stderr, "key:", err)
		os.Exit(2)
	}
	km, err := signer.NewKeyManager(keyBytes)
	if err != nil {
		fmt.Fprintln(os.Stderr, "keymanager:", err)
		os.Exit(2)
	}
	pk := km.PubKeyBytes()
	if err := schnorr.Validate(pk[:], msgHash, sig); err != nil {
		fmt.Println("VERIFY-FAIL:", err)
		os.Exit(1)
	}
	fmt.Println("VERIFY-OK pubkey=" + hexutil.Encode(pk[:]))
}
