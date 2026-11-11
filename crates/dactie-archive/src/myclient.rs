use dactie_utils::shared_structs::Instruction;
use std::error::Error;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// Reads operator commands from standard input. Every opening needs an explicit decision
/// per archive, so a threshold of archives, not the authority alone, controls de-anonymization.
pub(crate) async fn run(
    instruction_tx: mpsc::UnboundedSender<Instruction>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut lines = BufReader::new(io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let split: Vec<&str> = line.split_whitespace().collect();
        let instruction = match split.as_slice() {
            ["pending"] => Instruction::ListPendingOpens,
            ["approve", id] => Instruction::ApproveOpen { id: id.to_string() },
            ["deny", id] => Instruction::DenyOpen { id: id.to_string() },
            _ => {
                println!(
                    "Available Commands:
        - pending: Lists opening requests awaiting a decision
        - approve <id>: Contributes this archive's partial opening
        - deny <id>: Discards the request"
                );
                continue;
            }
        };
        instruction_tx.send(instruction)?;
    }
    log::info!("stdin closed, operator console ended");
    Ok(())
}
