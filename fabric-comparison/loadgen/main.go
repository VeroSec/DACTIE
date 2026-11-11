// Load generator for the Fabric side of the comparison with DACTIE's rate sweep
// (scripts/rate_sweep.sh). It submits transactions at a fixed arrival rate (open loop: each
// transaction starts on schedule, whether or not earlier ones have finished), each writing
// one record under a new key, and timestamps every transaction when its block reaches the
// receiver, a peer on another host, from proposal to commit as in Thakkar et al. Both
// timestamps are taken in this process, so the hosts' clocks do not matter.
//
// The receipts file has the columns of DACTIE's raw.csv (rate, receiver, received_at, seq,
// latency_ms) plus the block number, so scripts/rate_sweep_summary.py summarises both
// systems the same way. Transactions that fail or never commit go to the failures file and
// count as loss.
package main

import (
	"context"
	"crypto/x509"
	"encoding/csv"
	"flag"
	"fmt"
	"log"
	"os"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

	"github.com/hyperledger/fabric-gateway/pkg/client"
	"github.com/hyperledger/fabric-gateway/pkg/identity"
	"github.com/hyperledger/fabric-protos-go-apiv2/peer"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
)

type inflightTx struct {
	seq   int
	start time.Time
}

// output serialises writes from the submitting goroutines and the block listener.
type output struct {
	mu       sync.Mutex
	receipts *csv.Writer
	failures *csv.Writer
	rate     string
	receiver string
}

func (o *output) receipt(at time.Time, seq int, latency time.Duration, block uint64) {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.receipts.Write([]string{o.rate, o.receiver, at.UTC().Format("2006-01-02T15:04:05.000000Z"),
		strconv.Itoa(seq), strconv.FormatInt(latency.Milliseconds(), 10), strconv.FormatUint(block, 10)})
}

func (o *output) failure(seq int, stage, detail string) {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.failures.Write([]string{o.rate, strconv.Itoa(seq), stage, detail})
}

func (o *output) flush() {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.receipts.Flush()
	o.failures.Flush()
}

func csvFile(path string, header []string) *csv.Writer {
	f, err := os.Create(path)
	if err != nil {
		log.Fatalf("create %s: %v", path, err)
	}
	w := csv.NewWriter(f)
	w.Write(header)
	return w
}

func dial(addr, caPath string) *grpc.ClientConn {
	pem, err := os.ReadFile(caPath)
	if err != nil {
		log.Fatalf("read %s: %v", caPath, err)
	}
	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(pem) {
		log.Fatalf("no certificate in %s", caPath)
	}
	conn, err := grpc.NewClient(addr, grpc.WithTransportCredentials(credentials.NewClientTLSFromCert(pool, "")))
	if err != nil {
		log.Fatalf("connect %s: %v", addr, err)
	}
	return conn
}

func main() {
	rate := flag.Float64("rate", 25, "transactions per second")
	duration := flag.Duration("duration", 100*time.Second, "how long to submit")
	drain := flag.Duration("drain", 60*time.Second, "how long to wait for outstanding commits after the last submission")
	payloadFile := flag.String("payload-file", "", "record to store in every transaction (default: -payload-bytes of 'x')")
	payloadBytes := flag.Int("payload-bytes", 20, "record size when no -payload-file is given")
	run := flag.String("run", "", "key prefix, unique per run (default: start time and rate)")
	gatewayAddr := flag.String("gateway", "", "gateway peer host:port (endorsement and submission)")
	gatewayCA := flag.String("gateway-tls-ca", "", "TLS CA certificate of the gateway peer")
	eventsAddr := flag.String("events", "", "receiver peer host:port, whose committed blocks are timed")
	eventsCA := flag.String("events-tls-ca", "", "TLS CA certificate of the receiver peer")
	receiver := flag.String("receiver", "peer0.org1", "receiver name in the receipts file")
	mspID := flag.String("msp", "Org1MSP", "client MSP ID")
	certPath := flag.String("cert", "", "client certificate (PEM)")
	keyPath := flag.String("key", "", "client private key (PEM)")
	channel := flag.String("channel", "dactiebench", "channel")
	chaincode := flag.String("chaincode", "dactiebench", "chaincode name")
	receiptsPath := flag.String("out", "receipts.csv", "receipts file")
	failuresPath := flag.String("failures", "failures.csv", "failures file")
	flag.Parse()
	if *rate <= 0 || *gatewayAddr == "" || *eventsAddr == "" || *certPath == "" || *keyPath == "" {
		flag.Usage()
		os.Exit(2)
	}

	payload := []byte(nil)
	if *payloadFile != "" {
		var err error
		if payload, err = os.ReadFile(*payloadFile); err != nil {
			log.Fatalf("read payload: %v", err)
		}
	} else {
		payload = make([]byte, *payloadBytes)
		for i := range payload {
			payload[i] = 'x'
		}
	}
	runID := *run
	if runID == "" {
		runID = fmt.Sprintf("%s-r%g", time.Now().UTC().Format("20060102T150405"), *rate)
	}

	certPEM, err := os.ReadFile(*certPath)
	if err != nil {
		log.Fatalf("read certificate: %v", err)
	}
	cert, err := identity.CertificateFromPEM(certPEM)
	if err != nil {
		log.Fatalf("certificate: %v", err)
	}
	id, err := identity.NewX509Identity(*mspID, cert)
	if err != nil {
		log.Fatalf("identity: %v", err)
	}
	keyPEM, err := os.ReadFile(*keyPath)
	if err != nil {
		log.Fatalf("read key: %v", err)
	}
	key, err := identity.PrivateKeyFromPEM(keyPEM)
	if err != nil {
		log.Fatalf("key: %v", err)
	}
	sign, err := identity.NewPrivateKeySign(key)
	if err != nil {
		log.Fatalf("signer: %v", err)
	}

	gw, err := client.Connect(id, client.WithSign(sign), client.WithClientConnection(dial(*gatewayAddr, *gatewayCA)),
		client.WithEndorseTimeout(60*time.Second), client.WithSubmitTimeout(60*time.Second))
	if err != nil {
		log.Fatalf("gateway: %v", err)
	}
	defer gw.Close()
	events, err := client.Connect(id, client.WithSign(sign), client.WithClientConnection(dial(*eventsAddr, *eventsCA)))
	if err != nil {
		log.Fatalf("receiver: %v", err)
	}
	defer events.Close()

	out := &output{
		receipts: csvFile(*receiptsPath, []string{"rate", "receiver", "received_at", "seq", "latency_ms", "block"}),
		failures: csvFile(*failuresPath, []string{"rate", "seq", "stage", "detail"}),
		rate:     strconv.FormatFloat(*rate, 'f', -1, 64),
		receiver: *receiver,
	}

	var inflight sync.Map // transaction ID -> inflightTx
	var pending, committed, invalid, failed atomic.Int64

	ctx, cancel := context.WithCancel(context.Background())
	blocks, err := events.GetNetwork(*channel).FilteredBlockEvents(ctx)
	if err != nil {
		log.Fatalf("block events: %v", err)
	}
	listener := make(chan struct{})
	go func() {
		defer close(listener)
		for block := range blocks {
			now := time.Now()
			for _, tx := range block.GetFilteredTransactions() {
				v, ok := inflight.LoadAndDelete(tx.GetTxid())
				if !ok {
					continue
				}
				t := v.(inflightTx)
				pending.Add(-1)
				if tx.GetTxValidationCode() == peer.TxValidationCode_VALID {
					committed.Add(1)
					out.receipt(now, t.seq, now.Sub(t.start), block.GetNumber())
				} else {
					invalid.Add(1)
					out.failure(t.seq, "validation", tx.GetTxValidationCode().String())
				}
			}
		}
	}()
	time.Sleep(2 * time.Second) // the block stream is open before the first transaction

	contract := gw.GetNetwork(*channel).GetContract(*chaincode)
	interval := time.Duration(float64(time.Second) / *rate)
	n := int(duration.Seconds() * *rate)
	var submitters sync.WaitGroup
	start := time.Now()
	log.Printf("submitting %d transactions at %g/s, %d-byte records, run %s", n, *rate, len(payload), runID)
	for seq := 0; seq < n; seq++ {
		if d := time.Until(start.Add(time.Duration(seq) * interval)); d > 0 {
			time.Sleep(d)
		}
		submitters.Add(1)
		go func(seq int) {
			defer submitters.Done()
			t0 := time.Now()
			proposal, err := contract.NewProposal("put",
				client.WithBytesArguments([]byte(fmt.Sprintf("%s-%d", runID, seq)), payload))
			if err != nil {
				failed.Add(1)
				out.failure(seq, "proposal", err.Error())
				return
			}
			txID := proposal.TransactionID()
			inflight.Store(txID, inflightTx{seq: seq, start: t0})
			pending.Add(1)
			stage := "endorse"
			tx, err := proposal.Endorse()
			if err == nil {
				stage = "submit"
				_, err = tx.Submit()
			}
			if err != nil {
				if _, ok := inflight.LoadAndDelete(txID); ok {
					pending.Add(-1)
				}
				failed.Add(1)
				out.failure(seq, stage, err.Error())
			}
		}(seq)
	}
	offered := time.Since(start)
	submitters.Wait()
	deadline := time.Now().Add(*drain)
	for pending.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(200 * time.Millisecond)
	}
	cancel()
	<-listener
	lost := 0
	inflight.Range(func(k, v any) bool {
		lost++
		out.failure(v.(inflightTx).seq, "no-commit", k.(string))
		return true
	})
	out.flush()
	log.Printf("offered %d in %.1f s (%.1f/s): committed %d, invalid %d, failed %d, not committed after %s %d",
		n, offered.Seconds(), float64(n)/offered.Seconds(), committed.Load(), invalid.Load(), failed.Load(), *drain, lost)
}
