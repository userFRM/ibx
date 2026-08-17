//! What changed in the last day, put to the venue.
//!
//! Everything here is a preview or a resting order far from the market. It
//! places nothing that can fill at a price anyone would regret.

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
    println!("connected, account {}", client.account_id);

    let spy = client.qualify(Contract::stock("SPY"))?;
    println!("[ok] qualify -> con_id {}", spy.con_id);

    // 1. Tag 168. Never sent by this client before today.
    let mut delayed = Order::limit("BUY", 1.0, 1.00);
    delayed.good_after_time = "20260818 09:30:00".into();
    match client.what_if(&spy, &delayed) {
        Ok(state) => println!("[ok] good_after_time (tag 168) accepted, margin {}", state.init_margin_change),
        Err(why) => println!("[!!] good_after_time REFUSED BY VENUE: {why}"),
    }

    // 2. Fields the Python side used to drop, now on the wire. Sent from the
    //    Rust side, which is the same builder underneath.
    let mut carrying = Order::limit("BUY", 1.0, 1.00);
    carrying.clearing_intent = "IB".into();
    carrying.not_held = false;
    carrying.rule80a = "I".into();
    carrying.open_close = "O".into();
    match client.what_if(&spy, &carrying) {
        Ok(state) => println!("[ok] previously-dropped fields accepted, margin {}", state.init_margin_change),
        Err(why) => println!("[!!] previously-dropped fields REFUSED: {why}"),
    }

    // 3. The new refusals must fire, and must not fire on an ordinary order.
    let mut bad_tif = Order::limit("BUY", 1.0, 1.00);
    bad_tif.tif = "gtc".into();
    match client.place_order(9_000_001, &spy, &bad_tif) {
        Err(why) if why.message.contains("tif") => println!("[ok] a misspelled tif is refused here, not sent"),
        other => println!("[!!] a misspelled tif was not refused: {other:?}"),
    }
    let plain = Order::limit("BUY", 1.0, 1.00);
    match client.what_if(&spy, &plain) {
        Ok(_) => println!("[ok] an ordinary order is not caught by the new refusals"),
        Err(why) => println!("[!!] an ordinary order was REFUSED: {why}"),
    }

    // 4. A market that is open right now, to prove this is not a US-hours fluke.
    let eurusd = client.qualify(Contract::forex("EUR", "USD"))?;
    client.watch(&eurusd)?;
    std::thread::sleep(Duration::from_secs(8));
    match client.ticker(&eurusd) {
        Some(q) if q.bid > 0 => println!("[ok] EUR/USD live: bid {} ask {}", q.bid, q.ask),
        _ => println!("[..] EUR/USD no quote yet"),
    }
    match client.bars(&eurusd, "1 D", "1 hour") {
        Ok(bars) => println!("[ok] {} hourly bars for EUR/USD (midpoint, not trades)", bars.len()),
        Err(why) => println!("[!!] EUR/USD bars: {why}"),
    }
    match client.bars(&spy, "1 D", "1 hour") {
        Ok(bars) => println!("[ok] {} hourly bars for SPY (trades)", bars.len()),
        Err(why) => println!("[!!] SPY bars: {why}"),
    }

    // A bracket, which the engine could place and no caller could ask for
    // until today. Far enough from the market that nothing fills.
    match client.place_bracket(&spy, "BUY", 1.0, 1.00, 2.00, 0.50) {
        Ok([parent, tp, sl]) => {
            println!("[ok] bracket placed: parent {parent}, take-profit {tp}, stop {sl}");
            std::thread::sleep(std::time::Duration::from_secs(5));
            for id in [parent, tp, sl] {
                match client.trade(id) {
                    Some(t) => println!("     {id} -> {}", t.status.status),
                    None => println!("     {id} -> nothing said yet"),
                }
            }
            let _ = client.cancel_order(parent);
        }
        Err(why) => println!("[!!] bracket REFUSED: {why}"),
    }

    // 5. The session keeps what it is told.
    println!("[ok] session holds {} positions, {} account values, {} trades",
             client.positions().len(), client.account_values().len(), client.trades().len());

    client.disconnect();
    Ok(())
}
