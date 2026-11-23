use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};

use std::io;

fn main() {
    let mut client = DiscordIpcClient::new("1574266970763694212");

    println!("Connecting to Discord RPC...");

    if let Err(error) = client.connect() {
        eprintln!("RPC connection failed: {error}");
        return;
    }

    println!("RPC connected!");

    let activity = activity::Activity::new()
        .details("Training Special Week")
        .state("SPD 425 · STA 310 · POW 285 · GUT 190 · WIT 350");

    if let Err(error) = client.set_activity(activity) {
        eprintln!("Failed to set activity: {error}");
        let _ = client.close();
        return;
    }

    println!("Activity sent!");
    println!("Press ENTER to disconnect.");

    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);

    let _ = client.close();
}
