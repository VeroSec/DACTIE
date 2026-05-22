use tokio::io::{self, AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::mpsc;
use crate::abstswarm::{Notification};
use dactie_utils::shared_structs::Instruction;


/// The `NetworkClient` ireads user input from standard input and transforms it
/// into `Instruction`s. It then sends the `Instruction` on the mpsc channel that the `Network`
/// listens to.
///
/// It receives `Notification`s from the `Network` on the notification channel.
pub struct MyClient {
    instruction_tx: mpsc::UnboundedSender<Instruction>,
    notification_rx: mpsc::UnboundedReceiver<Notification>,
    stdin: Lines<BufReader<Stdin>>,
}

impl MyClient {
    pub fn new(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        notification_rx: mpsc::UnboundedReceiver<Notification>,

    ) -> MyClient {
        MyClient {
            instruction_tx,
            notification_rx,
            stdin: BufReader::new(io::stdin()).lines(),
        }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
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
        if input.starts_with("req_open") && split.len() > 1{
            let signatur = hex::decode(split[1])?;
            Self::send_req_open(signatur, self.instruction_tx.clone()).await?;
        } else if input.starts_with("open") && split.len() > 1 {
            let signatur = hex::decode(split[1])?;
            Self::send_open(signatur, self.instruction_tx.clone()).await?;
        } else if input.starts_with("end") {
            Self::send_end_init_archive(self.instruction_tx.clone()).await?;
        } else {
        println!("Available Commands:
        - req_open <signatur>: Requests open pairings from archives
        - open <signatur>: Opens Signatur   
        - end_init_archive: Ends Init Phase no new archives can join")
        }

        Ok(())
    }



    async fn handle_notification(
        notification: Notification,
    ) -> anyhow::Result<()> {
        match notification {
            Notification::Err(error) => {
                match error {
                    _ => log::error!("Received error from network: {error}")
                }
            }
        }

        Ok(())
    }
    async fn send_req_open(
        signature: Vec<u8>,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::ReqOpen {signature};

        instruction_tx.send(instruction)?;

        log::info!("Open sent to Network");

        Ok(())
    }

    async fn send_open(
        signature: Vec<u8>,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::Open {signature};

        instruction_tx.send(instruction)?;

        log::info!("Open sent to Network");

        Ok(())
    }

    async fn send_end_init_archive(
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> anyhow::Result<()> {
        let instruction = Instruction::EndInitArchive;

        instruction_tx.send(instruction)?;

        log::info!("End Init sent to Network");

        Ok(())
    }
}
