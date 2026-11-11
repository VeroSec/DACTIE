// Chaincode for the comparison with DACTIE: "put <key> <value>" stores one record under a
// key, the Fabric counterpart of sharing one CTI message. It runs as an external service
// (chaincode as a service) next to the peers of a host, see ../network/compose.yaml.
package main

import (
	"log"
	"os"

	"github.com/hyperledger/fabric-chaincode-go/v2/shim"
	"github.com/hyperledger/fabric-protos-go-apiv2/peer"
)

type store struct{}

func (store) Init(shim.ChaincodeStubInterface) *peer.Response { return shim.Success(nil) }

func (store) Invoke(stub shim.ChaincodeStubInterface) *peer.Response {
	args := stub.GetArgs()
	if len(args) != 3 || string(args[0]) != "put" {
		return shim.Error("usage: put <key> <value>")
	}
	if err := stub.PutState(string(args[1]), args[2]); err != nil {
		return shim.Error(err.Error())
	}
	return shim.Success(nil)
}

func main() {
	server := &shim.ChaincodeServer{
		CCID:     os.Getenv("CHAINCODE_ID"),
		Address:  os.Getenv("CHAINCODE_SERVER_ADDRESS"),
		CC:       store{},
		TLSProps: shim.TLSProperties{Disabled: true},
	}
	if err := server.Start(); err != nil {
		log.Fatalf("chaincode server: %v", err)
	}
}
