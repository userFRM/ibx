//! The session as something you hold.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_session

use std::env;
use std::time::Duration;

use ibx::types::model::{Contract, Order};
use ibx::{Client, Config};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect(&Config {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })?;

    let spy = client.qualify(Contract::stock("SPY"))?;

    // A quote exists because something is watching it, and updates itself
    // afterwards. Reading one waits on nothing.
    let stream = client.watch(&spy)?;
    std::thread::sleep(Duration::from_secs(3));
    if let Some(quote) = client.ticker(&spy) {
        println!("bid {} ask {}", quote.bid, quote.ask);
    }

    // The order is the thing you hold. Its number is bookkeeping this keeps.
    let order = client.place(&spy, &Order::limit("BUY", 1.0, 1.00))?;
    order.wait_done(Duration::from_secs(10));
    println!("order {} is {}", order.id(), order.status());
    if !order.is_done() {
        order.cancel()?;
    }

    // What the session holds, without asking for any of it.
    for position in client.positions() {
        println!("{:<8} {:>10}", position.contract.symbol, position.quantity);
    }
    for value in client.account_values().iter().filter(|v| v.tag == "NetLiquidation") {
        println!("{:<20} {} {}", value.tag, value.value, value.currency);
    }
    println!("{} orders, {} fills", client.trades().len(), client.fills().len());

    client.client().cancel_mkt_data(stream)?;
    client.disconnect();
    Ok(())
}
