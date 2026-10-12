use std::str::FromStr;

use crate::abstswarm::Notification;
use dactie_utils::mls_wrapper::MyOpenMls;
use std::sync::Arc;
use dactie_utils::shared_structs::{Instruction, MessageBody};
use libp2p::PeerId;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{self, AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::mpsc;

/// The `NetworkClient` ireads user input from standard input and transforms it
/// into `Instruction`s. It then sends the `Instruction` on the mpsc channel that the `Network`
/// listens to.
///
/// It receives `Notification`s from the `Network` on the notification channel.
pub(crate) struct MyClient {
    instruction_tx: mpsc::UnboundedSender<Instruction>,
    notification_rx: mpsc::UnboundedReceiver<Notification>,
    stdin: Lines<BufReader<Stdin>>,
    /// Shared with the swarm. Only read, to tell whether this peer has joined the
    /// common group yet; publishing before then cannot succeed.
    #[cfg_attr(not(feature = "bench"), allow(dead_code))]
    my_open_mls: Arc<MyOpenMls>,
    peer_id: PeerId,
}

impl MyClient {
    pub(crate) fn new(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        notification_rx: mpsc::UnboundedReceiver<Notification>,
        peer_id: PeerId,
        my_open_mls: Arc<MyOpenMls>,
    ) -> MyClient {
        MyClient {
            instruction_tx,
            notification_rx,
            stdin: BufReader::new(io::stdin()).lines(),
            peer_id,
            my_open_mls,
        }
    }

    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        log::info!("Start network client event loop.");

        // Optional synthetic load generator, compiled in only with --features bench.
        // Its traffic takes the normal path, so it is encrypted on the wire like any
        // other broadcast; the receiving peer must be subscribed to the topic.
        // Runtime env vars:
        // BENCH_PUBLISH=true|1 to enable publishing from this peer
        // BENCH_RATE=40 messages per second (default 40)
        // BENCH_TOPIC=Test topic to publish to (default "Test")
        // BENCH_PAYLOAD_BYTES=0 additional payload size (default 0)
        // BENCH_PAYLOAD_FILE=<path> send this file's contents as the body instead
        //   of synthetic padding, so the run carries a real CTI event
        // BENCH_READY_TIMEOUT=300 seconds to wait for registration (default 300)
        #[cfg(feature = "bench")]
        if std::env::var("BENCH_PUBLISH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            let instruction_tx = self.instruction_tx.clone();
            let my_open_mls = self.my_open_mls.clone();
            let rate: u64 = std::env::var("BENCH_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(40);
            let topic: String = std::env::var("BENCH_TOPIC").unwrap_or_else(|_| "Test".to_string());
            // Read once, not per message: re-reading at 40 msg/s would measure
            // the filesystem rather than the network.
            let payload_file = std::env::var("BENCH_PAYLOAD_FILE").ok().filter(|p| !p.is_empty());
            let file_body: Option<Vec<u8>> = match &payload_file {
                Some(path) => match std::fs::read(path) {
                    Ok(bytes) => {
                        log::info!(
                            "BENCH_PAYLOAD_FILE {} loaded: {} bytes",
                            path,
                            bytes.len()
                        );
                        Some(bytes)
                    }
                    Err(e) => {
                        log::error!("BENCH_PAYLOAD_FILE {} unreadable: {}", path, e);
                        return Ok(());
                    }
                },
                None => None,
            };
            let extra_bytes: usize = std::env::var("BENCH_PAYLOAD_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            // period = 1 / rate
            let period = if rate == 0 {
                Duration::from_secs(1)
            } else {
                Duration::from_nanos(1_000_000_000u64 / rate)
            };
            let ready_timeout: u64 = std::env::var("BENCH_READY_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);

            tokio::spawn(async move {
                let mut seq: u64 = 0;
                let mut ticker = tokio::time::interval(period);
                // Avoid immediate burst from first tick
                ticker.tick().await;

                // Publishing before this peer has joined the common group cannot
                // succeed: send_to_topic encrypts against it, so every attempt
                // fails with CommonGroup Not Set. Wait for registration rather
                // than emitting an error burst that pollutes the measurement.
                let waited = {
                    let started = SystemTime::now();
                    let mut poll = tokio::time::interval(Duration::from_millis(500));
                    let mut announced = false;
                    loop {
                        if my_open_mls.get_common_group_id().is_ok() {
                            break started.elapsed().unwrap_or(Duration::from_millis(0));
                        }
                        let elapsed = started.elapsed().unwrap_or(Duration::from_millis(0));
                        if elapsed >= Duration::from_secs(ready_timeout) {
                            log::error!(
                                "BENCH_PUBLISH giving up: no common group after {}s. \
                                 Register this peer with `register_identity <authority> <name>`.",
                                ready_timeout
                            );
                            return;
                        }
                        if !announced {
                            log::info!(
                                "BENCH_PUBLISH waiting for registration before publishing..."
                            );
                            announced = true;
                        }
                        poll.tick().await;
                    }
                };

                log::info!(
                    "BENCH_PUBLISH enabled: rate={} msg/s topic='{}' extra_bytes={} (ready after {:.1}s)",
                    rate,
                    topic,
                    extra_bytes,
                    waited.as_secs_f64()
                );
                loop {
                    ticker.tick().await;
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::from_millis(0));
                    let now_ms = now.as_millis();
                    // Format: B|<epoch_ms>|<seq>|<padding>. The originating peer is
                    // deliberately not in the payload: broadcasting it would reveal
                    // the sender to every recipient.
                    let mut msg = format!("B|{}|{}|", now_ms, seq).into_bytes();
                    match &file_body {
                        // The receiver parses only the three header fields, so a
                        // body containing '|' is harmless.
                        Some(body) => msg.extend_from_slice(body),
                        None => {
                            if extra_bytes > 0 {
                                // ensure deterministic payload size
                                msg.extend(std::iter::repeat(b'x').take(extra_bytes));
                            }
                        }
                    }
                    seq = seq.wrapping_add(1);

                    let instruction = Instruction::Send {
                        message: MessageBody::Broadcast {
                            topic: topic.clone(),
                            data: msg,
                        },
                    };
                    if let Err(e) = instruction_tx.send(instruction) {
                        log::error!("BENCH_PUBLISH failed to send instruction: {}", e);
                        break;
                    }
                }
            });
        }
        // Optional archive-query driver, compiled in only with --features bench.
        // Issues update_groups against an archive at a fixed rate so query latency
        // can be measured the same way publish latency is.
        // BENCH_ARCHIVE_QUERY=true|1  enable
        // BENCH_ARCHIVE_PEER=<peer_id> archive to query (required)
        // BENCH_ARCHIVE_RATE=1        queries per second (default 1)
        #[cfg(feature = "bench")]
        if std::env::var("BENCH_ARCHIVE_QUERY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            match std::env::var("BENCH_ARCHIVE_PEER")
                .ok()
                .and_then(|v| PeerId::from_str(&v).ok())
            {
                None => log::error!(
                    "BENCH_ARCHIVE_QUERY set but BENCH_ARCHIVE_PEER is missing or not a PeerId"
                ),
                Some(archive) => {
                    let instruction_tx = self.instruction_tx.clone();
                    let my_open_mls = self.my_open_mls.clone();
                    let rate: u64 = std::env::var("BENCH_ARCHIVE_RATE")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1);
                    let period = if rate == 0 {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_nanos(1_000_000_000u64 / rate)
                    };
                    tokio::spawn(async move {
                        // Same reason as the publisher: a peer with no common group
                        // has no groups to ask about, so the query is meaningless.
                        let mut poll = tokio::time::interval(Duration::from_millis(500));
                        while my_open_mls.get_common_group_id().is_err() {
                            poll.tick().await;
                        }
                        log::info!(
                            "BENCH_ARCHIVE_QUERY enabled: {} query/s against {}",
                            rate,
                            archive
                        );
                        let mut ticker = tokio::time::interval(period);
                        ticker.tick().await;
                        loop {
                            ticker.tick().await;
                            if let Err(e) =
                                instruction_tx.send(Instruction::UpdateGroups { peer_id: archive })
                            {
                                log::error!("BENCH_ARCHIVE_QUERY failed to send: {}", e);
                                break;
                            }
                        }
                    });
                }
            }
        }

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

    async fn handle_user_input(&mut self, input: &str) -> anyhow::Result<()> {
        let split: Vec<&str> = input.split_whitespace().collect();
        if input.starts_with("sub") && split.len() > 1 {
            let topic = split[1].to_string();
            Self::send_subscribe(topic, self.instruction_tx.clone()).await?;
        } else if input.starts_with("unsub") && split.len() > 1 {
            let topic = split[1].to_string();
            Self::send_unsubscribe(topic, self.instruction_tx.clone()).await?;
        } else if input.starts_with("attr_broad") && split.len() > 2 {
            let message = MessageBody::Broadcast {
                topic: split[1].to_string(),
                data: split[2..].join(" ").into_bytes(),
            };
            self.instruction_tx
                .send(Instruction::SendAttributed { message })?;
        } else if input.starts_with("attr_enc_broad") && split.len() > 2 {
            let message = MessageBody::Group {
                group_number: split[1].parse::<usize>()?,
                data: split[2..].join(" ").into_bytes(),
            };
            self.instruction_tx
                .send(Instruction::SendAttributed { message })?;
        } else if input.starts_with("broad") && split.len() > 2 {
            let topic = split[1];
            let message = split[2..].join(" ");
            Self::send_broadcast(topic.to_string(), message, self.instruction_tx.clone()).await?;
        } else if input.starts_with("req_kps") && split.len() > 1 {
            let res_ids: Result<Vec<PeerId>, _> =
                split[1..].iter().map(|&s| PeerId::from_str(s)).collect();
            let peer_ids = match res_ids {
                Ok(ids) => ids,
                Err(_) => {
                    println!("Failed to convert some of the peer IDs");
                    Vec::new()
                }
            };
            if !peer_ids.is_empty() {
                let instruction_tx = self.instruction_tx.clone();
                tokio::spawn(async { Self::request_keypackages(peer_ids, instruction_tx).await });
            }
        } else if input.starts_with("create_group") && split.len() > 1 {
            let res_ids: Result<Vec<PeerId>, _> =
                split[1..].iter().map(|&s| PeerId::from_str(s)).collect();
            let peer_ids = match res_ids {
                Ok(ids) => {
                    println!("Successfully converted: {ids:?}");
                    ids
                }
                Err(_) => {
                    println!("Failed to convert some of the peer IDs");
                    Vec::new()
                }
            };
            if !peer_ids.is_empty() {
                let instruction_tx = self.instruction_tx.clone();
                tokio::spawn(async { Self::create_group(peer_ids, instruction_tx).await });
            }
        } else if input.starts_with("enc_broad") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            let message = split[2..].join(" ");
            Self::send_enc_broadcast(group_number, message, self.instruction_tx.clone()).await?;
        } else if input.starts_with("ls_groups") {
            Self::list_groups(self.instruction_tx.clone()).await?;
        } else if input.starts_with("add_group_member") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            match PeerId::from_str(split[2]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::add_group_member(self.instruction_tx.clone(), group_number, peer_id)
                        .await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("remove_group_member") && split.len() > 2 {
            let group_number = split[1].parse::<usize>()?;
            match PeerId::from_str(split[2]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::remove_group_member(self.instruction_tx.clone(), group_number, peer_id)
                        .await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("update_key_material") && split.len() > 1 {
            let group_number = split[1].parse::<usize>()?;
            Self::update_key_material(self.instruction_tx.clone(), group_number).await?;
        } else if input.starts_with("delete_group_for_all") && split.len() > 1 {
            let group_number = split[1].parse::<usize>()?;
            Self::delete_group_for_all(self.instruction_tx.clone(), group_number).await?;
        } else if input.starts_with("update_groups") && split.len() > 1 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::update_groups(self.instruction_tx.clone(), peer_id).await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("peer_id") {
            println!("PeerID: {}", self.peer_id)
        } else if input.starts_with("register_identity") && split.len() > 2 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::register_identity(
                        self.instruction_tx.clone(),
                        peer_id,
                        split[2].to_string(),
                    )
                    .await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("peer_from_sig") && split.len() > 2 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::peer_from_sig(self.instruction_tx.clone(), peer_id, split[2].to_string())
                        .await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("renew") && split.len() > 2 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => self.instruction_tx.send(Instruction::RenewPeer {
                    peer_id,
                    sig_file_path: split[2].to_string(),
                })?,
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else if input.starts_with("get_opener_keys") && split.len() > 1 {
            match PeerId::from_str(split[1]) {
                Ok(peer_id) => {
                    println!("Successfully converted: {peer_id:?}");
                    Self::get_opener_keys(self.instruction_tx.clone(), peer_id).await?;
                }
                Err(_) => println!("Failed to convert some of the peer IDs"),
            };
        } else {
            println!("Available Commands:
        - ls_groups: Lists all available groups and prints the group_number + group ids
        - req_kps <peer_id_1>, <peer_id_2>, ...: Requests kps from the peers in the arguments
        - create_group <peer_id_1>, <peer_id_2>, ...: Creates a group with the mentioned peers
        - enc_broad <group_number> <msg>: Sends an encrypted and signed message to the group
        - sub <topic>: Subscribes to a topic
        - unsub <topic>: Unsubscribes from a topic
        - broad <topic> <msg>: Sends a public available message to the group
        - attr_broad <topic> <msg>: Like broad, but receivers see this member's MemberID
        - attr_enc_broad <group_number> <msg>: Like enc_broad, but receivers see this member's MemberID
        - add_group_member <group_number> <peer_id>: Adds a new Group Member
        - remove_group_member <group_number> <peer_id>: Removes peer from group, except group owner,
        - delete_group_for_all <group_number>: Deletes Group for all peers, only callable by owner
        - update_key_material <group_number>: Updates own key material in group
        - update_groups <archive_peer_id>: Requests updates for all groups from an archive
        - peer_id: Prints out PeerId
        - register_identity <authorization_peer_id> <identity>: Registers a new identity at Authorization Authority
        - peer_from_sig <authorization_peer_id> <save_file_path>: Adds a new peer from an existing GroupSignature Memberkey
        - get_opener_keys <authorization_peer_id>: Requests Opener Keys from Authority
        - renew <authorization_peer_id> <save_file_path>: Keeps this sharing peer registered in the next epoch, using a slot of the member whose key file is save_file_path")
        }

        Ok(())
    }

    async fn handle_notification(notification: Notification) -> anyhow::Result<()> {
        match notification {
            Notification::Data {
                propagation_source: peer_id,
                message_body,
                signature,
                attributed_to,
            } => match message_body {
                MessageBody::Broadcast { topic, data } => {
                    let s = String::from_utf8_lossy(&data);
                    if s.starts_with("B|") {
                        // Parse B|<epoch_ms>|<seq>|...
                        let mut parts = s.split('|');
                        let _ = parts.next();
                        let ts_ms = parts
                            .next()
                            .and_then(|v| v.parse::<u128>().ok())
                            .unwrap_or(0);
                        let seq = parts
                            .next()
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(0);
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or(Duration::from_millis(0))
                            .as_millis();
                        let latency_ms = now.saturating_sub(ts_ms);
                        println!("BENCH_RX topic={topic} via={peer_id} seq={seq} latency_ms={} sig={}", latency_ms, hex::encode(&signature));
                    } else {
                        println!("Got Broadcastmessage: \n'{}'\nTopic: '{topic}'\nPropagating Peer: {peer_id}\n{}\nSignature:\n{}", s, sender(attributed_to), hex::encode(&signature));
                    }
                }
                MessageBody::Group { group_number, data } => {
                    let s = String::from_utf8_lossy(&data);
                    if s.starts_with("B|") {
                        let mut parts = s.split('|');
                        let _ = parts.next();
                        let ts_ms = parts
                            .next()
                            .and_then(|v| v.parse::<u128>().ok())
                            .unwrap_or(0);
                        let seq = parts
                            .next()
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(0);
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or(Duration::from_millis(0))
                            .as_millis();
                        let latency_ms = now.saturating_sub(ts_ms);
                        println!("BENCH_RX group={} via={peer_id} seq={seq} latency_ms={} sig={}", group_number, latency_ms, hex::encode(&signature));
                    } else {
                        println!("Got Groupmessage: \n'{}'\nGroup number: '{group_number}'\nPropagating Peer: {peer_id}\n{}\nSignature:\n{}", s, sender(attributed_to), hex::encode(&signature));
                    }
                }
                MessageBody::GroupEnc { .. } | MessageBody::BroadcastEnc { .. } => {
                    log::error!("Client received notification with Encrypted Messages")
                }
                MessageBody::Proposal { .. } => {
                    log::debug!("Received Add Group Member Proposal")
                }
                MessageBody::Commit { group_id, .. } => {
                    log::debug!("Received Commit for Groupid {group_id:?}")
                }
            },
            Notification::Err(error) => {
                log::error!("Received error from network: {error}")
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
                data: data.as_bytes().to_vec(),
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
                data: data.as_bytes().to_vec(),
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

    async fn list_groups(instruction_tx: mpsc::UnboundedSender<Instruction>) -> anyhow::Result<()> {
        let instruction = Instruction::ListGroups;
        instruction_tx.send(instruction)?;
        log::info!("List Groups Instruction sent");

        Ok(())
    }

    async fn add_group_member(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::AddGroupMember {
            group_number,
            peer_id,
        };
        instruction_tx.send(instruction)?;
        log::info!("Add Group Member Instruction sent to Abstswarm");

        Ok(())
    }

    async fn remove_group_member(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::RemoveGroupMember {
            group_number,
            peer_id,
        };
        instruction_tx.send(instruction)?;
        log::info!("Remove Group Member Instruction sent to Abstswarm");

        Ok(())
    }

    async fn delete_group_for_all(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::DeleteGroupForAll { group_number };
        instruction_tx.send(instruction)?;
        log::info!("Delete Group For All Instruction sent to Abstswarm");

        Ok(())
    }

    async fn update_key_material(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        group_number: usize,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::UpdateKeyMaterial { group_number };
        instruction_tx.send(instruction)?;
        log::info!("Update Key Material Instruction sent to Abstswarm");

        Ok(())
    }

    async fn update_groups(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::UpdateGroups { peer_id };
        instruction_tx.send(instruction)?;
        log::info!("Update Groups Instruction sent to Abstswarm");

        Ok(())
    }

    async fn register_identity(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
        identity: String,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::RegisterIdentity { peer_id, identity };
        instruction_tx.send(instruction)?;
        log::info!("RegisterIdentity Instruction sent to Abstswarm");

        Ok(())
    }

    async fn peer_from_sig(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
        sig_file_path: String,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::PeerFromSig {
            peer_id,
            sig_file_path,
        };
        instruction_tx.send(instruction)?;
        log::info!("PeerFromSig Instruction sent to Abstswarm");

        Ok(())
    }

    async fn get_opener_keys(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::GetOpenerKeys { peer_id };
        instruction_tx.send(instruction)?;
        log::info!("GetOpenerKeys Instruction sent to Abstswarm");

        Ok(())
    }
}

/// "Attributed to member X" for attributed messages, whose signature proves the MemberID.
fn sender(attributed_to: Option<usize>) -> String {
    match attributed_to {
        Some(id) => format!("Attributed to member: {id}"),
        None => "Sender: anonymous".to_string(),
    }
}
