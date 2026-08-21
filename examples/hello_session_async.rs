//! The same session, from inside a runtime.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --features async --example hello_session_async

use std::env;
use std::time::Duration;

use ibx::types::model::{Contract, Order};
use ibx::{AsyncClient, Config};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AsyncClient::connect(Config {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })
    .await?;

    let spy = client.qualify(Contract::stock("SPY")).await?;

    // Watching sends and returns, so it is not awaited. Neither is reading the
    // quote it produces: that is a memory read.
    client.watch(&spy)?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Some(quote) = client.ticker(&spy) {
        println!("bid {} ask {}", quote.bid, quote.ask);
    }

    let order = client.place(&spy, &Order::limit("BUY", 1.0, 1.00)).await?;
    client.wait_done(&order, Duration::from_secs(10)).await;
    println!("order {} is {}", order.id(), order.status());
    if !order.is_done() {
        order.cancel()?;
    }

    for position in client.positions() {
        println!("{:<8} {:>10}", position.contract.symbol, position.quantity);
    }

    client.disconnect().await;
    Ok(())
}
