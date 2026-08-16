//! The short way to ask this client a question.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_client

use std::env;
use std::time::Duration;

use ibx::types::model::{Contract, Order};
use ibx::{Client, EClientConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ib = Client::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })?;

    let spy = ib.qualify(Contract::stock("SPY"))?;
    println!("SPY is contract {}", spy.con_id);

    let bars = ib.bars(&spy, "2 D", "1 hour")?;
    if let Some(last) = bars.last() {
        println!("{} bars, last closed at {}", bars.len(), last.close);
    }

    // A quote exists because something subscribed to it. Reading it does not
    // wait on the callback loop, so this thread may read as often as it likes.
    let stream = ib.watch(&spy)?;
    for _ in 0..25 {
        if let Some(q) = ib.quote_of(&spy)
            && q.bid > 0
        {
            println!("bid {} ask {}", q.bid, q.ask);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    ib.cancel_mkt_data(stream)?;

    let preview = ib.preview(&spy, &Order::limit("BUY", 1.0, 1.00))?;
    println!("that order would cost {}", preview.commission_and_fees);

    for value in ib.summary()? {
        println!("{:<22} {} {}", value.tag, value.value, value.currency);
    }

    ib.disconnect();
    Ok(())
}
