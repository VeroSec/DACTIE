// Anonymous Fabric clients for the load generator (-idemix-dir). Every transaction carries a
// fresh Idemix pseudonym with its own proof of membership, so two transactions of the same
// member cannot be linked, as with DACTIE's group signatures, where every message gets a
// fresh signature.
//
// Fabric's own Idemix MSP (github.com/IBM/idemix, the version linked into Fabric 2.5.16)
// derives one pseudonym when it is set up and signs every transaction with it, which makes
// a client pseudonymous, not anonymous. newIdentity repeats exactly what that MSP's Setup
// does (idemixmsp.go: Nym derivation, then the association proof with OU and role disclosed,
// enrollment ID and revocation handle hidden), and sign is its IdemixSigningIdentity.Sign
// (a signature with the pseudonym's key). The peers and the orderer verify both with the
// unchanged Fabric MSP.
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"

	idemix "github.com/IBM/idemix/bccsp"
	"github.com/IBM/idemix/bccsp/keystore"
	"github.com/IBM/idemix/bccsp/schemes/dlog/crypto/translator/amcl"
	bccsp "github.com/IBM/idemix/bccsp/types"
	im "github.com/IBM/idemix/idemixmsp"
	math "github.com/IBM/mathlib"
	protov1 "github.com/golang/protobuf/proto"
	"github.com/hyperledger/fabric-protos-go-apiv2/msp"
	"google.golang.org/protobuf/proto"
)

// Attribute positions in an Idemix credential, as in IBM/idemix idemixmsp.go.
const (
	rhIndex  = 3
	eidIndex = 2
)

// idemixMember is one member's credential, read from idemixgen's output directory
// (msp/IssuerPublicKey and user/SignerConfig).
type idemixMember struct {
	mspID   string
	csp     bccsp.BCCSP
	ipk     bccsp.Key
	rpk     bccsp.Key // revocation public key, for checking proofs as a verifier does
	userKey bccsp.Key
	signer  *im.IdemixMSPSignerConfig
}

// idemixIdentity is one single-use pseudonym: the serialized identity a transaction carries
// as its creator, and the pseudonym's key for signing it.
type idemixIdentity struct {
	member *idemixMember
	creds  []byte // marshalled SerializedIdemixIdentity
	nym    bccsp.Key
}

func (id *idemixIdentity) MspID() string       { return id.member.mspID }
func (id *idemixIdentity) Credentials() []byte { return id.creds }

// sign signs the whole message: the Idemix MSP hashes it itself (client.WithHash(hash.NONE)).
func (id *idemixIdentity) sign(message []byte) ([]byte, error) {
	return id.member.csp.Sign(id.member.userKey, message,
		&bccsp.IdemixNymSignerOpts{Nym: id.nym, IssuerPK: id.member.ipk})
}

func loadIdemixMember(dir, mspID string) (*idemixMember, error) {
	// FP256BN with the AMCL translator: the curve of IBM/idemix.NewIdemixMsp, which is the
	// Idemix MSP Fabric 2.5 uses.
	curve := math.Curves[math.FP256BN_AMCL]
	csp, err := idemix.New(&keystore.Dummy{}, curve, &amcl.Fp256bn{C: curve}, true)
	if err != nil {
		return nil, fmt.Errorf("idemix provider: %w", err)
	}
	ipkBytes, err := os.ReadFile(filepath.Join(dir, "msp", "IssuerPublicKey"))
	if err != nil {
		return nil, err
	}
	ipk, err := csp.KeyImport(ipkBytes, &bccsp.IdemixIssuerPublicKeyImportOpts{
		Temporary:      true,
		AttributeNames: []string{"OU", "Role", "EnrollmentID", "RevocationHandle"},
	})
	if err != nil {
		return nil, fmt.Errorf("issuer public key: %w", err)
	}
	rpkBytes, err := os.ReadFile(filepath.Join(dir, "msp", "RevocationPublicKey"))
	if err != nil {
		return nil, err
	}
	rpk, err := csp.KeyImport(rpkBytes, &bccsp.IdemixRevocationPublicKeyImportOpts{Temporary: true})
	if err != nil {
		return nil, fmt.Errorf("revocation public key: %w", err)
	}
	signerBytes, err := os.ReadFile(filepath.Join(dir, "user", "SignerConfig"))
	if err != nil {
		return nil, err
	}
	signer := &im.IdemixMSPSignerConfig{}
	if err := protov1.Unmarshal(signerBytes, signer); err != nil {
		return nil, fmt.Errorf("signer config: %w", err)
	}
	userKey, err := csp.KeyImport(signer.Sk, &bccsp.IdemixUserSecretKeyImportOpts{Temporary: true})
	if err != nil {
		return nil, fmt.Errorf("user secret key: %w", err)
	}
	return &idemixMember{mspID: mspID, csp: csp, ipk: ipk, rpk: rpk, userKey: userKey, signer: signer}, nil
}

// newIdentity derives a fresh pseudonym and proves, bound to it, that its holder has a
// credential of this MSP with the signer's OU and the member role.
func (m *idemixMember) newIdentity() (*idemixIdentity, error) {
	nym, err := m.csp.KeyDeriv(m.userKey, &bccsp.IdemixNymKeyDerivationOpts{Temporary: true, IssuerPK: m.ipk})
	if err != nil {
		return nil, fmt.Errorf("derive pseudonym: %w", err)
	}
	nymPublic, err := nym.PublicKey()
	if err != nil {
		return nil, err
	}
	proof, err := m.csp.Sign(m.userKey, nil, &bccsp.IdemixSignerOpts{
		Credential: m.signer.Cred,
		Nym:        nym,
		IssuerPK:   m.ipk,
		Attributes: []bccsp.IdemixAttribute{
			{Type: bccsp.IdemixBytesAttribute},  // OU, disclosed
			{Type: bccsp.IdemixIntAttribute},    // role, disclosed
			{Type: bccsp.IdemixHiddenAttribute}, // enrollment ID
			{Type: bccsp.IdemixHiddenAttribute}, // revocation handle
		},
		RhIndex:  rhIndex,
		EidIndex: eidIndex,
		CRI:      m.signer.CredentialRevocationInformation,
	})
	if err != nil {
		return nil, fmt.Errorf("association proof: %w", err)
	}
	raw, err := nymPublic.Bytes()
	if err != nil {
		return nil, err
	}
	ou, err := proto.Marshal(&msp.OrganizationUnit{
		MspIdentifier:                m.mspID,
		OrganizationalUnitIdentifier: m.signer.OrganizationalUnitIdentifier,
		CertifiersIdentifier:         m.ipk.SKI(),
	})
	if err != nil {
		return nil, err
	}
	role, err := proto.Marshal(&msp.MSPRole{MspIdentifier: m.mspID, Role: msp.MSPRole_MEMBER})
	if err != nil {
		return nil, err
	}
	creds, err := protov1.Marshal(&im.SerializedIdemixIdentity{
		NymX: raw[:len(raw)/2], NymY: raw[len(raw)/2:], Ou: ou, Role: role, Proof: proof,
	})
	if err != nil {
		return nil, err
	}
	return &idemixIdentity{member: m, creds: creds, nym: nym}, nil
}

// identityPool derives n single-use identities on all cores before the timed run, so that
// deriving them does not limit the arrival rate. It returns them and the mean CPU time one
// derivation took (wall time per identity times cores in use).
func (m *idemixMember) identityPool(n int) ([]*idemixIdentity, time.Duration, error) {
	pool := make([]*idemixIdentity, n)
	workers := runtime.GOMAXPROCS(0)
	var wg sync.WaitGroup
	errs := make(chan error, workers)
	next := make(chan int, n)
	for i := 0; i < n; i++ {
		next <- i
	}
	close(next)
	start := time.Now()
	for w := 0; w < workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range next {
				id, err := m.newIdentity()
				if err != nil {
					errs <- err
					return
				}
				pool[i] = id
			}
		}()
	}
	wg.Wait()
	close(errs)
	if err := <-errs; err != nil {
		return nil, 0, err
	}
	perIdentity := time.Since(start) * time.Duration(workers) / time.Duration(max(n, 1))
	return pool, perIdentity, nil
}

// benchIdemix measures, on one core, what an anonymous transaction costs the client (a fresh
// identity, one signature) and what it costs every verifier: checking the identity's proof
// of membership as Fabric's MSP does (Validate, a pairing check) and the signature (Verify).
// A transaction is checked by the endorser, the orderer and every committing peer.
func benchIdemix(m *idemixMember, n, messageBytes int) error {
	message := make([]byte, messageBytes)
	var derive, sign, validate, verify time.Duration
	size := 0
	for i := 0; i < n; i++ {
		t0 := time.Now()
		id, err := m.newIdentity()
		if err != nil {
			return err
		}
		t1 := time.Now()
		sig, err := id.sign(message)
		if err != nil {
			return err
		}
		t2 := time.Now()
		serialized := &im.SerializedIdemixIdentity{}
		if err := protov1.Unmarshal(id.creds, serialized); err != nil {
			return err
		}
		ok, err := m.csp.Verify(m.ipk, serialized.Proof, nil, &bccsp.IdemixSignerOpts{
			RevocationPublicKey: m.rpk,
			Attributes: []bccsp.IdemixAttribute{
				{Type: bccsp.IdemixBytesAttribute, Value: []byte(m.signer.OrganizationalUnitIdentifier)},
				{Type: bccsp.IdemixIntAttribute, Value: 1}, // member role, as getIdemixRoleFromMSPRole
				{Type: bccsp.IdemixHiddenAttribute},
				{Type: bccsp.IdemixHiddenAttribute},
			},
			RhIndex:  rhIndex,
			EidIndex: eidIndex,
			Epoch:    0,
		})
		if err != nil || !ok {
			return fmt.Errorf("proof %d does not verify: %v", i, err)
		}
		t3 := time.Now()
		nymPublic, err := m.csp.KeyImport(append(append([]byte{}, serialized.NymX...), serialized.NymY...),
			&bccsp.IdemixNymPublicKeyImportOpts{Temporary: true})
		if err != nil {
			return err
		}
		ok, err = m.csp.Verify(nymPublic, sig, message, &bccsp.IdemixNymSignerOpts{IssuerPK: m.ipk})
		if err != nil || !ok {
			return fmt.Errorf("signature %d does not verify: %v", i, err)
		}
		t4 := time.Now()
		derive += t1.Sub(t0)
		sign += t2.Sub(t1)
		validate += t3.Sub(t2)
		verify += t4.Sub(t3)
		size = len(id.creds)
	}
	ms := func(d time.Duration) float64 { return float64(d.Microseconds()) / float64(n) / 1000 }
	fmt.Printf("idemix cost on one core, mean of %d: client: fresh identity %.2f ms, signature over %d bytes %.3f ms; "+
		"verifier: identity proof %.2f ms, signature %.3f ms; serialized identity %d bytes\n",
		n, ms(derive), messageBytes, ms(sign), ms(validate), ms(verify), size)
	return nil
}
