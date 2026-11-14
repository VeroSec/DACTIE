#!/bin/bash
# Channel and chaincode setup, run by deploy.sh inside a fabric-tools container with host
# networking and build/ mounted at /work: the orderer joins channel "dactiebench", every peer
# joins and installs the chaincode package, both orgs approve the chaincode with the
# endorsement policy OR(Org1 peer, Org2 peer) (one endorsement suffices), and Org1 commits it.
set -euo pipefail
cd /work
set -a
# shellcheck source=/dev/null  # written by generate.sh into build/
source hosts.env
# shellcheck source=/dev/null  # written by deploy.sh into build/
source chaincode.env
set +a
C=/work/crypto
ORDERER_CA=$C/ordererOrganizations/example.com/tlsca/tlsca.example.com-cert.pem
ORDERER_ADMIN=$C/ordererOrganizations/example.com/users/Admin@example.com/tls
POLICY="OR('Org1MSP.peer','Org2MSP.peer')"
export CORE_PEER_TLS_ENABLED=true

as_admin_of() { # org ip port
    export CORE_PEER_LOCALMSPID=Org${1}MSP
    export CORE_PEER_MSPCONFIGPATH=$C/peerOrganizations/org$1.example.com/users/Admin@org$1.example.com/msp
    export CORE_PEER_TLS_ROOTCERT_FILE=$C/peerOrganizations/org$1.example.com/tlsca/tlsca.org$1.example.com-cert.pem
    export CORE_PEER_ADDRESS=$2:$3
}

# shellcheck disable=SC2153  # ORDERER_IP comes from hosts.env
osnadmin channel join --channelID dactiebench --config-block /work/channel.block -o "$ORDERER_IP:7053" \
    --ca-file "$ORDERER_CA" --client-cert "$ORDERER_ADMIN/client.crt" --client-key "$ORDERER_ADMIN/client.key"
sleep 5 # Raft elects the single orderer as leader

for peer in "1 $P0ORG1_IP 7051" "2 $P0ORG2_IP 8051" "1 $P1ORG1_IP 9051" "2 $P1ORG2_IP 10051"; do
    as_admin_of "$peer"
    peer channel join -b /work/channel.block
    peer lifecycle chaincode install /work/dactiebench.tgz
done

for peer in "1 $P0ORG1_IP 7051" "2 $P0ORG2_IP 8051"; do
    as_admin_of "$peer"
    peer lifecycle chaincode approveformyorg -o "$ORDERER_IP:7050" --tls --cafile "$ORDERER_CA" \
        --channelID dactiebench --name dactiebench --version 1 --sequence 1 \
        --package-id "$CHAINCODE_ID" --signature-policy "$POLICY"
done

as_admin_of 1 "$P0ORG1_IP" 7051
peer lifecycle chaincode commit -o "$ORDERER_IP:7050" --tls --cafile "$ORDERER_CA" \
    --channelID dactiebench --name dactiebench --version 1 --sequence 1 --signature-policy "$POLICY" \
    --peerAddresses "$P0ORG1_IP:7051" --tlsRootCertFiles $C/peerOrganizations/org1.example.com/tlsca/tlsca.org1.example.com-cert.pem \
    --peerAddresses "$P0ORG2_IP:8051" --tlsRootCertFiles $C/peerOrganizations/org2.example.com/tlsca/tlsca.org2.example.com-cert.pem
peer lifecycle chaincode querycommitted --channelID dactiebench --name dactiebench
