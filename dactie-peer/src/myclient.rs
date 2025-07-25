use std::str::FromStr;

use libp2p::PeerId;
use tokio::io::{self, AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::mpsc;
use crate::abstswarm::{Notification};
use dactie_utils::shared_structs::{MessageBody, Instruction};


/// The `NetworkClient` ireads user input from standard input and transforms it
/// into `Instruction`s. It then sends the `Instruction` on the mpsc channel that the `Network`
/// listens to.
///
/// It receives `Notification`s from the `Network` on the notification channel.
pub(crate) struct MyClient {
    instruction_tx: mpsc::UnboundedSender<Instruction>,
    notification_rx: mpsc::UnboundedReceiver<Notification>,
    stdin: Lines<BufReader<Stdin>>,
    peer_id: PeerId
}

impl MyClient {
    pub(crate) fn new(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        notification_rx: mpsc::UnboundedReceiver<Notification>,
        peer_id: PeerId

    ) -> MyClient {
        MyClient {
            instruction_tx,
            notification_rx,
            stdin: BufReader::new(io::stdin()).lines(),
            peer_id
        }
    }

    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        log::info!("Start network client event loop.");
        loop {
            tokio::select! {
                Ok(Some(line)) = self.stdin.next_line() => {
                    Self::handle_user_input(&mut self, &line).await?
                }
                Some(notification) = self.notification_rx.recv() =>  {
                     Self::handle_notification(notification).await?
                }
                else => {
                    log::info!("Both stdin and notification channel closed. Ending client");
                    break Ok(())

                }
            }
        }
    }

    async fn handle_user_input(
        &mut self,
        input: &str,
    ) -> anyhow::Result<()> {
        let split: Vec<&str> = input.split_whitespace().collect();
        if input.starts_with("sub") && split.len() > 1{
            let topic = split[1].to_string();
            Self::send_subscribe(topic, self.instruction_tx.clone()).await?;
        } else if input.starts_with("unsub") && split.len() > 1 {
            let topic = split[1].to_string();
            Self::send_unsubscribe(topic, self.instruction_tx.clone()).await?;
        } else if input.starts_with("broad") && split.len() > 2 {
            let topic = split[1];
            let message = split[2..].join(" ");
            Self::send_broadcast(topic.to_string(),message, self.instruction_tx.clone()).await?;
        } else if input.starts_with("req_kps") && split.len() > 1 {
            let res_ids: Result<Vec<PeerId>,_> = split[1..].iter().map(|&s| PeerId::from_str(s)).collect();
            let peer_ids = match res_ids {
                Ok(ids) => {ids},
                Err(_) => {
                    println!("Failed to convert some of the peer IDs");
                Vec::new()},
            };
            if !peer_ids.is_empty() {
                let instruction_tx = self.instruction_tx.clone();
                tokio::spawn(async {Self::request_keypackages(peer_ids, instruction_tx).await});
            }
        } else if input.starts_with("create_group") && split.len() > 1 {
            let res_ids: Result<Vec<PeerId>,_> = split[1..].iter().map(|&s| PeerId::from_str(s)).collect();
            let peer_ids = match res_ids {
                Ok(ids) => {println!("Successfully converted: {:?}", ids);
                    ids},
                Err(_) => {
                    println!("Failed to convert some of the peer IDs");
                    Vec::new()},
            };
            if !peer_ids.is_empty() {
                let instruction_tx = self.instruction_tx.clone();
                tokio::spawn(async {Self::create_group(peer_ids,instruction_tx).await});
            }
        } else if input.starts_with("enc_broad") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            let message = split[2..].join(" ");
            Self::send_enc_broadcast(group_number,message, self.instruction_tx.clone()).await?;

        } else if input.starts_with("ls_groups") {
            Self::list_groups(self.instruction_tx.clone()).await?;
        }else if input.starts_with("add_group_member") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            match PeerId::from_str(split[2]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::add_group_member(self.instruction_tx.clone(),group_number,peer_id).await?;
                    },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };

        }else if input.starts_with("remove_group_member") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            match PeerId::from_str(split[2]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::remove_group_member(self.instruction_tx.clone(),group_number,peer_id).await?;
                },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };

        }else if input.starts_with("update_key_material") && split.len() > 1{
            let group_number = split[1].parse::<usize>()?;
            Self::update_key_material(self.instruction_tx.clone(),group_number).await?;
        } else if input.starts_with("delete_group_for_all") && split.len() > 1{
            let group_number = split[1].parse::<usize>()?;
            Self::delete_group_for_all(self.instruction_tx.clone(),group_number).await?;
        }else if input.starts_with("update_groups") && split.len() > 1{
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::update_groups(self.instruction_tx.clone(),peer_id).await?;
                },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };

        }else if input.starts_with("peer_id"){
            println!("PeerID: {}", self.peer_id)
        }else if input.starts_with("register_identity") && split.len() > 2 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::register_identity(self.instruction_tx.clone(),peer_id,split[2].to_string()).await?;
                },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };
        }else if input.starts_with("peer_from_sig") && split.len() > 2 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::peer_from_sig(self.instruction_tx.clone(),peer_id,split[2].to_string()).await?;
                },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };
        }else if input.starts_with("get_opener_keys") && split.len() > 1 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {println!("Successfully converted: {:?}", peer_id);
                    Self::get_opener_keys(self.instruction_tx.clone(),peer_id).await?;
                },
                Err(_) => println!("Failed to convert some of the peer IDs")
            };
        }else {
        println!("Available Commands:
        - ls_groups: Lists all available groups and prints the group_number + group ids
        - req_kps <peer_id_1>, <peer_id_2>, ...: Requests kps from the peers in the arguments
        - create_group <peer_id_1>, <peer_id_2>, ...: Creates a group with the mentioned peers
        - enc_broad <group_number> <msg>: Sends an encrypted and signed message to the group
        - sub <topic>: Subscribes to a topic
        - unsub <topic>: Unsubscribes from a topic
        - broad <topic> <msg>: Sends a public available message to the group
        - add_group_member <group_number> <peer_id>: Adds a new Group Member
        - remove_group_member <group_number> <peer_id>: Removes peer from group, except group owner,
        - delete_group_for_all <group_number>: Deletes Group for all peers, only callable by owner
        - update_key_material <group_number>: Updates own key material in group
        - update_groups <archive_peer_id>: Requests updates for all groups from an archive
        - peer_id: Prints out PeerId
        - register_identity <authorization_peer_id> <identity>: Registers a new identity at Authorization Authority
        - peer_from_sig <authorization_peer_id> <save_file_path>: Adds a new peer from an existing GroupSignature Memberkey
        - get_opener_keys <authorization_peer_id>: Requests Opener Keys from Authority")
        }

        Ok(())
    }



    async fn handle_notification(
        notification: Notification,
    ) -> anyhow::Result<()> {
        match notification {
            Notification::Data{propagation_source: peer_id, message_body , signature, ..} => {
                match message_body {
                    MessageBody::Broadcast {topic, data} => {
                        println!("Got Broadcastmessage: \n'{}'\nTopic: '{topic}'\nPropagating Peer: {peer_id}\nSignature:\n{}",String::from_utf8_lossy(&data),  hex::encode(&signature));
                    }
                    MessageBody::Group {group_number, data} => {
                        println!("Got Groupmessage: \n'{}'\nGroup number: '{group_number}'\nPropagating Peer: {peer_id}\nSignature:\n{}",String::from_utf8_lossy(&data),  hex::encode(&signature));
                    }
                    MessageBody::GroupEnc {..} | MessageBody::BroadcastEnc{..} => {
                        log::error!("Client received notification with Encrypted Messages")
                    }
                   MessageBody::Proposal {..} => {
                       log::debug!("Received Add Group Member Proposal")
                   }
                    MessageBody::Commit {group_id, ..} => {
                        log::debug!("Received Commit for Groupid {group_id:?}")
                    }
                }

            }
            Notification::Err(error) => {
                match error {
                    _ => log::error!("Received error from network: {error}")
                }
            }
        }

        Ok(())
    }

    async fn send_subscribe(
        topic: String,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::Subscribe(topic.as_bytes().to_vec());

        instruction_tx.send(instruction)?;

        log::info!("Subscribe sent to Network");

        Ok(())
    }

    async fn send_unsubscribe(
        topic: String,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::UnSubscribe(topic);
        instruction_tx.send(instruction)?;
        log::info!("UnSubscribe sent to Network");

        Ok(())
    }


    async fn send_broadcast(
        topic: String,
        data: String,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::Send {
            message: MessageBody::Broadcast {
                topic,
                data: data.as_bytes().to_vec()
            },
        };

        instruction_tx.send(instruction)?;

        log::info!("Broadcast sent to Network");

        Ok(())
    }

    async fn send_enc_broadcast(
        group_number: usize,
        data: String,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::Send {
            message: MessageBody::Group {
                group_number,
                data: data.as_bytes().to_vec()
            },
        };

        instruction_tx.send(instruction)?;

        log::info!("Broadcast sent to Network");

        Ok(())
    }


    async fn request_keypackages(
        peer_ids: Vec<PeerId>,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::RequestKPs(peer_ids);
        instruction_tx.send(instruction)?;
        log::info!("Requested Keypackages Instruction sent");

        Ok(())
    }

    async fn create_group(
        peer_ids: Vec<PeerId>,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::CreateGroup(peer_ids);
        instruction_tx.send(instruction)?;
        log::info!("Create Group Instruction sent");

        Ok(())
    }

    async fn list_groups(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::ListGroups;
        instruction_tx.send(instruction)?;
        log::info!("List Groups Instruction sent");

        Ok(())
    }

    async fn add_group_member(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
        peer_id: PeerId
    ) -> anyhow::Result<()> {
        let instruction = Instruction::AddGroupMember {group_number, peer_id};
        instruction_tx.send(instruction)?;
        log::info!("Add Group Member Instruction sent to Abstswarm");

        Ok(())
    }

    async fn remove_group_member(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
        peer_id: PeerId
    ) -> anyhow::Result<()> {
        let instruction = Instruction::RemoveGroupMember {group_number, peer_id};
        instruction_tx.send(instruction)?;
        log::info!("Remove Group Member Instruction sent to Abstswarm");

        Ok(())
    }

    async fn delete_group_for_all(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize
    ) -> anyhow::Result<()> {
        let instruction = Instruction::DeleteGroupForAll {group_number};
        instruction_tx.send(instruction)?;
        log::info!("Delete Group For All Instruction sent to Abstswarm");

        Ok(())
    }


    async fn update_key_material(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::UpdateKeyMaterial {group_number};
        instruction_tx.send(instruction)?;
        log::info!("Update Key Material Instruction sent to Abstswarm");

        Ok(())
    }

    async fn update_groups(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::UpdateGroups {peer_id};
        instruction_tx.send(instruction)?;
        log::info!("Update Groups Instruction sent to Abstswarm");

        Ok(())
    }

    async fn register_identity(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
        identity: String,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::RegisterIdentity {peer_id, identity};
        instruction_tx.send(instruction)?;
        log::info!("RegisterIdentity Instruction sent to Abstswarm");

        Ok(())
    }

    async fn peer_from_sig(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
        sig_file_path: String,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::PeerFromSig {peer_id, sig_file_path};
        instruction_tx.send(instruction)?;
        log::info!("PeerFromSig Instruction sent to Abstswarm");

        Ok(())
    }

    async fn get_opener_keys(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::GetOpenerKeys {peer_id};
        instruction_tx.send(instruction)?;
        log::info!("GetOpenerKeys Instruction sent to Abstswarm");

        Ok(())
    }

}
