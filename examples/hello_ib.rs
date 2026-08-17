//! The session as something you look at.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_ib

use std::env;
use std::time::Duration;

use ibx::types::model::{Contract, Order};
use ibx::{EClientConfig, IB};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ib = IB::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })?;

    let spy = ib.qualify(Contract::stock("SPY"))?;

    // A quote updates itself once something is watching it.
    let stream = ib.watch(&spy)?;
    ib.wait_on_update(Duration::from_secs(5));
    if let Some(quote) = ib.ticker(&spy) {
        println!("bid {} ask {}", quote.bid, quote.ask);
    }

    // An order well below the market, so it rests rather than fills.
    let trade = ib.place_order(&spy, &Order::limit("BUY", 1.0, 1.00))?;
    let id = trade.order.order_id;

    // Wait for the venue to say something about it, rather than for a while.
    ib.loop_until(Duration::from_secs(10), |ib| {
        ib.trade(id).is_some_and(|t| t.status.status != "PendingSubmit")
    });
    if let Some(trade) = ib.trade(id) {
        println!("order {id} is {} ({} filled)", trade.status.status, trade.status.filled);
    }
    ib.cancel_order(id)?;

    // What the session holds, without asking for any of it.
    for position in ib.positions() {
        println!("{:<8} {:>10}", position.contract.symbol, position.quantity);
    }
    for value in ib.account_values().iter().filter(|v| v.tag == "NetLiquidation") {
        println!("{:<20} {} {}", value.tag, value.value, value.currency);
    }
    println!("{} orders, {} fills", ib.trades().len(), ib.fills().len());

    ib.client().cancel_mkt_data(stream)?;
    ib.disconnect();
    Ok(())
}
