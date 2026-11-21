//! A2 Tune: TUNE <level> keys (PTT ON), TUNE OFF unkeys (PTT OFF), and
//! TUNE ? is answered the way MercuryChat's TUNE probe reads it.

use mercury_adapter::{Ack, Command, ControlEvent, TuneArg};
use mercury_contract::{attach, contract, next_event, Tag};

#[tokio::test(flavor = "multi_thread")]
async fn tune_on_keys_and_off_unkeys() {
    contract("tune_on_keys_and_off_unkeys", Tag::Both, |t| async move {
        let mut a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::Tune(TuneArg::Level(-20.0))).await, Ok(Ack::Ok));
        next_event(&mut a, t.reply(), "PTT ON for the tone", |e| *e == ControlEvent::Ptt(true)).await;
        assert_eq!(a.client.command(&Command::Tune(TuneArg::Off)).await, Ok(Ack::Ok));
        next_event(&mut a, t.reply(), "PTT OFF after TUNE OFF", |e| *e == ControlEvent::Ptt(false)).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_probe_answers() {
    contract("tune_probe_answers", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        // mc-core's probe_tune counts an answer as support and WRONG as none;
        // the adapter takes the TUNE <level> line as the answer.
        assert_eq!(a.client.command(&Command::Tune(TuneArg::Query)).await, Ok(Ack::Ok));
    })
    .await;
}
