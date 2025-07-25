use libp2p::PeerId;
use openmls::group::GroupId;
use serde::{Deserialize, Serialize};
use thressig::structs::{EncShare, Signature};

//Trait that defines a message before it has been signed with the group signature
pub trait Signable {
    type Output;
    fn new(body: Self, signature: Signature) -> Self::Output;
}

//Trait that defines a message combined with a group signature.
pub trait Verifiable{
    type Body;
    fn into_parts(self) -> (Self::Body, Option<Signature>);
}

//A direct request to another peer without using gossipsub
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request{
    pub body: RequestMessage,
    pub signature: Option<Signature>
}

impl Verifiable for Request {
    type Body = RequestMessage;
    fn into_parts(self) -> (Self::Body, Option<Signature>) {
        (self.body, self.signature)
    }
}

//A messages that is sent as response to a request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response{
    pub body: ResponseMessage,
    pub signature: Option<Signature>
}
impl Verifiable for Response {
    type Body = ResponseMessage;
    fn into_parts(self) -> (Self::Body, Option<Signature>) {
        (self.body,self.signature)
    }
}

// Different Types of Requests
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum RequestMessage{
    KeyPackage,
    JoinGroup {
        welcome: Vec<u8>,
        ratchet_tree: Vec<u8>
    },
    Update {
        group_id: Vec<u8>,
        epoch: u64
    },
    AddIDShare {
        id: usize,
        peer_id: PeerId,
        enc_share: EncShare,
        commits: Vec<thressig::G1>
    },
    AddPeerShare {
        peer_id: PeerId,
        enc_share: EncShare,
        commits: Vec<thressig::G1>
    },
    //In a practical setting this could be a manual process with in person meetings etc.
    VerifyIdentity {
        identity: String,
        with_signature: bool
    },
    JoinKeyMaterial {
        g_sk: thressig::G1,
        h_sk: thressig::G1,
        pi1: (thressig::FieldElement,thressig::FieldElement),
        pi2: (thressig::FieldElement,thressig::FieldElement),
        enc_shares: Vec<EncShare>,
        commits: Vec<thressig::G1>,
        keypackage: Vec<u8>
    },
    JoinKeyPackage {
        keypackage: Vec<u8>,
        elgamal_pk: thressig::G2,
    },
    AddPeerToSig {
        keypackage: Vec<u8>,
        enc_shares: Vec<EncShare>,
        commits: Vec<thressig::G1>,
    },
    RequestOpen {
        signature: Vec<u8>
    },
    RequestOpenerKeys,
}

impl Signable for RequestMessage {
    type Output = Request;
    fn new(body: Self, signature: Signature) -> Self::Output {
        Request { body, signature: Some(signature) }
    }
}

//Different Types of Responses
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ResponseMessage{
    KeyPackage(Vec<u8>),
    JoinGroup,
    ResUpdate {
        group_id: Vec<u8>,
        messages: Vec<MessageBody>,
    },
    IdentityVerified {
        id: usize,
        n: Option<thressig::G1>,
        group_key: Vec<u8>,
        opener_pubkeys: Option<Vec<thressig::G2>>//In theory the pubkeys should be fetched over another channel, to take power from the Authority
    },
    MemKey {
        partial_memkey: Option<Vec<u8>>,
    },
    PeerAdded,
    AddPeerShare,
    EncSharePairings{signature: Vec<u8>,
        enc_share_id_pairings: Vec<u8>,
        enc_share_peer_pairings: Vec<u8>,
    },
    OpenerKeys{
        opener_pubkeys: Vec<thressig::G2>
    }
}
impl Signable for ResponseMessage {
    type Output = Response;

    fn new(body: Self, signature: Signature) -> Self::Output {
        Response { body, signature:Some(signature) }
    }
}

// A Gossipsub message with group signature
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct  Message {
    pub body: MessageBody,
    pub signature: Signature
}

impl Verifiable for Message {
    type Body = MessageBody;
    fn into_parts(self) -> (Self::Body, Option<Signature>) {
        (self.body,Some(self.signature))
    }
}

// DIfferent Gossipsub message types
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum MessageBody {
    Broadcast { topic: String, data: Vec<u8>},
    BroadcastEnc { topic: String, data: Vec<u8>, nonce: Vec<u8>},
    GroupEnc { group_id: Vec<u8>, enc_data: Vec<u8> , nonce: Vec<u8>},
    Group { group_number: usize, data: Vec<u8>},
    Proposal {group_id: Vec<u8>, data: Vec<u8>, epoch: u64},
    Commit {group_id: Vec<u8>, commit: Vec<u8>, welcome_option: Option<Vec<u8>>, epoch: u64}
}

impl Signable for MessageBody {
    type Output = Message;

    fn new(body: Self, signature: Signature) -> Self::Output {
        Message { body, signature }
    }
}

/// An instruction for the swarm to do something. Most likely to send a message of some sort
#[derive(Debug, Clone)]
pub enum Instruction {
    /// Instruct the network to send a message
    Send {
        message: MessageBody,
    },
    Subscribe(Vec<u8>),
    UnSubscribe(String),
    RequestKPs(Vec<PeerId>),
    CreateGroup(Vec<PeerId>),
    AddGroupMember {
        group_number: usize,
        peer_id: PeerId,
    },
    RemoveGroupMember {
        group_number: usize,
        peer_id: PeerId,
    },
    DeleteGroupForAll {
        group_number: usize,
    },
    UpdateKeyMaterial {
        group_number: usize,
    },
    ListGroups,
    Commit {
        group_id: GroupId,
        peer_ids: Vec<PeerId>,
        commit: Vec<u8>,
        welcome: Option<Vec<u8>>,
        epoch: u64
    },
    UpdateGroups {
        peer_id: PeerId,
    },
    RegisterIdentity {
        peer_id: PeerId,
        identity: String
    },
    PeerFromSig {
        peer_id: PeerId,
        sig_file_path: String
    },
    GetOpenerKeys {
        peer_id: PeerId,
    },
    ReqOpen {
        signature: Vec<u8>
    },
    Open {
        signature: Vec<u8>
    },
    EndInitArchive,
}