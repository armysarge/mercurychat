//! M3a Task 6: periodic beacons (DSC-03): opt-in, jittered, idle-only, never
//! bursting, never armed by a restart or a recovery.
//!
//! `scheduler` drives the pure state machine with synthetic times. `service`
//! runs the app against the FakeNet modem on tokio's paused clock: setup is in
//! real time, then the clock is paused and moved by hand.

use mc_core::app::{BeaconScheduler, ModemState, NotConfirmedReason, Tick, BUSY_HOLD};
use std::time::Duration;
use tokio::time::Instant;

const MIN: Duration = Duration::from_secs(60);

mod scheduler {
    use super::*;

    /// A scheduler whose jitter draw is always `u`, armed at `t0` and idle.
    fn armed(u: f64, t0: Instant) -> BeaconScheduler {
        let mut s = BeaconScheduler::new(Box::new(move || u));
        s.on_state(ModemState::Ready, t0);
        s.arm(t0);
        s
    }

    #[test]
    fn jitter_bounds_are_85_to_115_percent_of_the_interval() {
        let t0 = Instant::now();
        assert_eq!(armed(0.0, t0).deadline(), Some(t0 + Duration::from_secs_f64(8.5 * 60.0)));
        let late = armed(0.999_999, t0).deadline().unwrap() - t0;
        assert!(late < Duration::from_secs_f64(11.5 * 60.0) && late > Duration::from_secs_f64(11.49 * 60.0));
        assert_eq!(armed(0.5, t0).deadline(), Some(t0 + 10 * MIN));
    }

    #[test]
    fn a_new_scheduler_is_disarmed_and_never_sends() {
        let t0 = Instant::now();
        let mut s = BeaconScheduler::new(Box::new(|| 0.5));
        s.on_state(ModemState::Ready, t0);
        assert!(!s.active());
        assert_eq!(s.deadline(), None);
        assert_eq!(s.tick(t0 + 100 * MIN), Tick::Nothing);
        assert!(s.suppressed(t0).is_some(), "the reason is given");
    }

    /// M4 B7: while a QSY owns the rig nothing is sent; afterwards the
    /// schedule resumes one interval on (the beacon is delayed, not lost).
    #[test]
    fn due_during_a_qsy_waits_for_it() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        s.on_qsy(true, t0 + 9 * MIN);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing, "due during the QSY: nothing is sent");
        assert_eq!(s.suppressed(t0 + 10 * MIN).as_deref(), Some("a QSY is under way"));
        let end = t0 + 11 * MIN;
        s.on_qsy(false, end);
        assert_eq!(s.deadline(), Some(end + 10 * MIN), "the schedule resumes");
        assert_eq!(s.suppressed(end), None);
    }

    #[test]
    fn due_during_calling_is_skipped_not_queued() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        s.on_state(ModemState::Calling, t0 + 9 * MIN);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing, "due while calling: nothing is sent");
        assert_eq!(s.deadline(), None, "and nothing waits for the call to end");
        assert_eq!(s.suppressed(t0 + 10 * MIN).as_deref(), Some("a call is being placed"));
        // The call ends one minute later: not an immediate beacon.
        let end = t0 + 11 * MIN;
        s.on_state(ModemState::Ready, end);
        assert_eq!(s.tick(end), Tick::Nothing);
        assert_eq!(s.deadline(), Some(end + 10 * MIN));
        assert_eq!(s.suppressed(end), None);
    }

    #[test]
    fn due_during_draining_then_session_ends_sends_once_after_an_interval() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        s.on_state(ModemState::Negotiating, t0 + 2 * MIN);
        s.on_state(ModemState::Connected, t0 + 3 * MIN);
        s.on_state(ModemState::Draining, t0 + 9 * MIN);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing);
        let end = t0 + 12 * MIN;
        s.on_state(ModemState::Ready, end);
        // Walk two intervals after the session: exactly one beacon, one interval in.
        let mut sent = Vec::new();
        let mut t = end;
        while t < end + 20 * MIN {
            if s.tick(t) == Tick::Send {
                sent.push(t);
            }
            t += Duration::from_secs(1);
        }
        assert_eq!(sent, [end + 10 * MIN], "one beacon, one interval after the session ended");
        // And the one after it is a whole interval later still, never back to back.
        assert_eq!(s.deadline(), Some(end + 20 * MIN));
    }

    #[test]
    fn nothing_is_sent_in_any_state_but_idle() {
        use ModemState::*;
        for state in [Unavailable, Attaching, Calling, Negotiating, Connected, Draining, Recovering, Faulted] {
            let t0 = Instant::now();
            let mut s = armed(0.5, t0);
            s.on_state(state, t0 + MIN);
            assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing, "{state:?}");
            assert_eq!(s.tick(t0 + 40 * MIN), Tick::Nothing, "{state:?}");
        }
    }

    #[test]
    fn faulted_or_disarmed_suppresses_with_reason() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        assert_eq!(s.suppressed(t0), None);
        s.on_state(ModemState::Faulted, t0);
        assert_eq!(s.suppressed(t0).as_deref(), Some("the modem is faulted"));
        s.on_state(ModemState::Ready, t0);
        s.disarm();
        let why = s.suppressed(t0).expect("a reason when disarmed");
        assert!(why.contains("off"), "{why}");
        // A modem that is not attached disarms too, and says so.
        let mut s = armed(0.5, t0);
        s.on_state(ModemState::Recovering, t0);
        assert!(!s.active());
        assert!(s.suppressed(t0).is_some());
        // Back to Ready does not bring it back.
        s.on_state(ModemState::Ready, t0 + MIN);
        assert!(!s.active());
        assert_eq!(s.tick(t0 + 60 * MIN), Tick::Nothing);
    }

    #[test]
    fn busy_holds_a_due_beacon_for_ten_seconds_only() {
        let t0 = Instant::now();
        // Busy for under 10 s when due: skipped, rescheduled from the hold's end.
        let mut s = armed(0.5, t0);
        let busy = t0 + 10 * MIN - Duration::from_secs(3);
        s.on_busy(true, busy);
        assert_eq!(s.suppressed(busy).as_deref(), Some("the channel is busy"));
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing);
        assert_eq!(s.deadline(), Some(busy + BUSY_HOLD + 10 * MIN));

        // Busy for over 10 s with no OFF: the due beacon goes out; still busy.
        let mut s = armed(0.5, t0);
        s.on_busy(true, t0 + MIN);
        assert_eq!(
            s.suppressed(t0 + MIN + BUSY_HOLD - Duration::from_millis(1)).as_deref(),
            Some("the channel is busy")
        );
        assert_eq!(s.suppressed(t0 + MIN + BUSY_HOLD), None, "the hold has elapsed");
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Send);
        assert!(s.busy_since().is_some(), "the channel still shows busy");
        // A second BUSY ON does not restart the hold clock.
        s.on_busy(true, t0 + 10 * MIN);
        assert_eq!(s.busy_since(), Some(t0 + MIN));
        s.on_busy(false, t0 + 11 * MIN);
        assert_eq!(s.busy_since(), None);
    }

    #[test]
    fn channel_is_unknown_until_the_first_busy_line() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        assert!(!s.channel_known());
        s.on_busy(false, t0);
        assert!(s.channel_known());
    }

    #[test]
    fn one_send_per_due_and_a_changed_interval_redraws() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Send);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Nothing, "no second beacon at once");
        assert_eq!(s.deadline(), Some(t0 + 20 * MIN));
        s.set_interval(5, t0 + 12 * MIN);
        assert_eq!(s.deadline(), Some(t0 + 12 * MIN + 5 * MIN));
    }

    const WINDOW: Duration = Duration::from_secs(15);

    #[test]
    fn two_silent_beacons_in_a_row_hold_the_schedule_with_the_reason() {
        let reason = NotConfirmedReason::BroadcastPortSilent.text();
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        assert_eq!(s.tick(t0 + 10 * MIN), Tick::Send);
        s.on_silent();
        assert!(!s.held(), "one silent beacon is not enough");
        assert_eq!(s.deadline(), Some(t0 + 20 * MIN));
        assert_eq!(s.suppressed(t0 + 10 * MIN + WINDOW), None);
        assert_eq!(s.tick(t0 + 20 * MIN), Tick::Send);
        s.on_silent();
        assert!(s.held());
        assert_eq!(s.deadline(), None, "held: nothing is scheduled");
        assert_eq!(s.suppressed(t0 + 21 * MIN).as_deref(), Some(reason));
        assert_eq!(s.tick(t0 + 100 * MIN), Tick::Nothing);
        // Nothing silently re-arms it: not a new interval, not idle again.
        s.set_interval(5, t0 + 30 * MIN);
        s.on_state(ModemState::Connected, t0 + 31 * MIN);
        s.on_state(ModemState::Listening, t0 + 32 * MIN);
        assert_eq!(s.deadline(), None);
        assert_eq!(s.suppressed(t0 + 33 * MIN).as_deref(), Some(reason));
        // A beacon that did go out (Beacon now) ends the hold.
        let t1 = t0 + 40 * MIN;
        s.on_transmitted(t1);
        assert!(!s.held());
        assert_eq!(s.deadline(), Some(t1 + 5 * MIN));
        assert_eq!(s.suppressed(t1), None);
    }

    #[test]
    fn rearming_ends_a_hold_and_a_transmitted_beacon_resets_the_count() {
        let t0 = Instant::now();
        let mut s = armed(0.5, t0);
        s.on_silent();
        s.on_transmitted(t0);
        s.on_silent();
        assert!(!s.held(), "not two in a row");
        s.on_silent();
        assert!(s.held());
        s.disarm();
        s.arm(t0 + MIN);
        assert!(!s.held());
        assert_eq!(s.deadline(), Some(t0 + 11 * MIN));
        s.on_silent();
        assert!(!s.held(), "the count starts again");
    }
}

#[cfg(feature = "sim")]
mod service {
    use super::*;
    use mc_core::app::{
        spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, BeaconStatus, BroadcastKind,
        BroadcastOutcome, ChannelState, CommandOk, EventEnvelope,
    };
    use mc_core::profile::{ModemTarget, StationProfile};
    use mc_storage::Store;
    use mch_protocol::beacon::{Beacon, CAP_CHAT};
    use mercury_adapter::fake::FakeNet;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio::sync::broadcast::Receiver;

    fn opts(root: &Path) -> AppOptions {
        AppOptions {
            sim_root: Some(root.to_path_buf()),
            stations_root: Some(root.join("stations")),
            ..AppOptions::default()
        }
    }

    fn profile(net: &FakeNet, grid: Option<&str>) -> StationProfile {
        let mut p = StationProfile::new_existing("ZS6ABC");
        p.modem = ModemTarget::Existing {
            host: "127.0.0.1".into(),
            control_port: net.a_base,
            data_port: net.a_base + 1,
            broadcast_port: Some(net.a_bcast),
            telemetry_url: None,
        };
        p.grid = grid.map(String::from);
        p
    }

    async fn state(rx: &mut Receiver<EventEnvelope>, want: ModemState) {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv()).await.expect("a modem state").unwrap();
            if let AppEvent::ModemState { state, detail } = ev.event {
                if state == want {
                    return;
                }
                assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
            }
        }
    }

    /// An app on `store` with `profile` selected and its modem Ready.
    async fn ready_on(store: Store, root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>) {
        let h = spawn_app(store, opts(root));
        let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
            CommandOk::Profile(p) => p.id,
            other => panic!("{other:?}"),
        };
        let mut rx = h.subscribe();
        h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
        state(&mut rx, ModemState::Ready).await;
        (h, rx)
    }

    async fn ready(root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>) {
        ready_on(Store::open_in_memory().unwrap(), root, profile).await
    }

    async fn snap(h: &AppHandle) -> AppSnapshot {
        match h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    async fn beacon(h: &AppHandle) -> BeaconStatus {
        snap(h).await.beacon
    }

    /// Let the actor, the modem task and the fake modem's sockets catch up
    /// with what has just happened, without moving the clock.
    async fn settle() {
        for _ in 0..40 {
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Move the paused clock by `by`, in steps, letting the real I/O settle.
    async fn advance(by: Duration) {
        let step = Duration::from_secs(30);
        let mut left = by;
        while !left.is_zero() {
            let d = left.min(step);
            tokio::time::advance(d).await;
            settle().await;
            left -= d;
        }
    }

    fn sent(net: &FakeNet) -> usize {
        net.broadcast_log().len()
    }

    async fn done(h: &AppHandle, cmd: AppCommand) {
        assert_eq!(h.call(cmd).await.unwrap(), CommandOk::Done);
    }

    fn conflict(r: Result<CommandOk, AppError>) -> String {
        match r {
            Err(AppError::Conflict { message }) => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn utc_in_minutes(s: &str) -> f64 {
        let t = chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc);
        (t - chrono::Utc::now()).num_milliseconds() as f64 / 60_000.0
    }

    #[tokio::test(flavor = "current_thread")]
    async fn beacons_off_by_default() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, Some("KG33"))).await;
        tokio::time::pause();
        let b = beacon(&h).await;
        assert!(!b.active && !b.preferred);
        assert_eq!((b.interval_min, b.include_grid), (10, true));
        assert_eq!((b.next_due_utc, b.last_utc, b.channel), (None, None, ChannelState::Unknown));
        assert!(b.suppressed.is_some(), "off, and it says so");
        advance(40 * MIN).await;
        assert_eq!(sent(&net), 0, "nothing is sent unless asked for");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_beacon_within_jitter_bounds() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), profile(&net, Some("KG33xx"))).await;
        tokio::time::pause();
        let t0 = Instant::now();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        let b = beacon(&h).await;
        assert!(b.active && b.preferred);
        let due = utc_in_minutes(b.next_due_utc.as_deref().expect("a due time"));
        assert!((8.4..=11.6).contains(&due), "first beacon due in {due} min");
        // The change was published.
        let mut published = false;
        while let Ok(ev) = rx.try_recv() {
            published |= matches!(ev.event, AppEvent::Beacon { status } if status.active);
        }
        assert!(published, "AppEvent::Beacon on arming");

        let mut when = None;
        while when.is_none() && Instant::now() - t0 < 13 * MIN {
            advance(Duration::from_secs(10)).await;
            if sent(&net) > 0 {
                when = Some(Instant::now() - t0);
            }
        }
        let when = when.expect("a beacon within 13 minutes");
        assert!(when >= Duration::from_secs_f64(8.5 * 60.0), "not before 8.5 min: {when:?}");
        assert!(when <= Duration::from_secs_f64(11.5 * 60.0) + Duration::from_secs(10), "{when:?}");
        let (side, payload) = net.broadcast_log().remove(0);
        assert_eq!(side, 0);
        let b = Beacon::decode(&payload).unwrap();
        assert_eq!((b.callsign.as_str(), b.grid.as_deref(), b.caps), ("ZS6ABC", Some("KG33xx"), CAP_CHAT));
        let s = beacon(&h).await;
        assert!(s.last_utc.is_some());
        let next = utc_in_minutes(s.next_due_utc.as_deref().unwrap());
        assert!((8.4..=11.6).contains(&next), "the next one is a whole interval away: {next}");
        // Over the next two intervals no more than two more, never two together.
        advance(25 * MIN).await;
        assert!((1..=3).contains(&(sent(&net) - 1)), "{}", sent(&net));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interval_is_validated_and_kept_per_profile() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, None)).await;
        for bad in [0, 4, 61, 1000] {
            match h.call(AppCommand::SetBeaconInterval { minutes: bad }).await {
                Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, "minutes"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        assert_eq!(beacon(&h).await.interval_min, 10);
        done(&h, AppCommand::SetBeaconInterval { minutes: 5 }).await;
        assert_eq!(beacon(&h).await.interval_min, 5);
        done(&h, AppCommand::SetBeaconInterval { minutes: 60 }).await;
        assert_eq!(beacon(&h).await.interval_min, 60);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn channel_unknown_until_first_busy_line() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, None)).await;
        tokio::time::pause();
        assert_eq!(beacon(&h).await.channel, ChannelState::Unknown);
        net.send_control(0, "BUSY OFF");
        settle().await;
        assert_eq!(beacon(&h).await.channel, ChannelState::Clear);
        net.send_control(0, "BUSY ON");
        settle().await;
        assert!(matches!(beacon(&h).await.channel, ChannelState::Busy { .. }));
        net.send_control(0, "BUSY OFF");
        settle().await;
        assert_eq!(beacon(&h).await.channel, ChannelState::Clear);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn busy_suppresses_then_hold_caps_at_10s() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), profile(&net, None)).await;
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        net.send_control(0, "BUSY ON");
        settle().await;
        let b = beacon(&h).await;
        assert!(matches!(b.channel, ChannelState::Busy { .. }));
        assert_eq!(b.suppressed.as_deref(), Some("the channel is busy"));
        advance(Duration::from_secs(9)).await;
        assert!(beacon(&h).await.suppressed.is_some(), "still held at 9 s");
        advance(Duration::from_secs(2)).await;
        let b = beacon(&h).await;
        assert_eq!(b.suppressed, None, "the hold ends after 10 s");
        assert!(matches!(b.channel, ChannelState::Busy { .. }), "the channel still shows busy");
        // The hold's end was published, not just readable.
        let mut last = None;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Beacon { status } = ev.event {
                last = Some(status);
            }
        }
        assert_eq!(last.expect("a beacon event").suppressed, None);
        // Busy with no OFF: the due beacon goes out.
        advance(12 * MIN).await;
        assert_eq!(sent(&net), 1);
        assert!(matches!(beacon(&h).await.channel, ChannelState::Busy { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recovery_disarms_beacons_and_keeps_preference() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), profile(&net, None)).await;
        done(&h, AppCommand::SetBeacon { on: true }).await;
        assert!(beacon(&h).await.active);
        // The clock runs for real until the modem is back: with it paused, the
        // five re-attach attempts (1 to 8 s apart) all ran within a few ms, before
        // the fake Mercury had let go of the killed connection (CI on Linux).
        net.kill_client(0); // Mercury closes our sockets
        state(&mut rx, ModemState::Recovering).await;
        let b = beacon(&h).await;
        assert!(!b.active, "recovery disarms");
        assert!(b.preferred, "but the preference stays");
        state(&mut rx, ModemState::Ready).await;
        let b = beacon(&h).await;
        assert!(!b.active && b.preferred && b.next_due_utc.is_none(), "{b:?}");
        tokio::time::pause();
        advance(35 * MIN).await;
        assert_eq!(sent(&net), 0, "a recovered modem is not beaconing until you say so");
        // Arming again is explicit and works.
        done(&h, AppCommand::SetBeacon { on: true }).await;
        assert!(beacon(&h).await.active);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_does_not_rearm_beacons() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mercurychat.db");
        let net = FakeNet::start().await;
        {
            let (h, _rx) = ready_on(Store::open(&db).unwrap(), dir.path(), profile(&net, None)).await;
            done(&h, AppCommand::SetBeacon { on: true }).await;
            assert!(beacon(&h).await.active);
        } // the app stops
          // Let it close its connections before the next app attaches: the
          // fake Mercury serves one host at a time, and under load the next
          // attach raced the teardown (the gate, 18 Nov).
        tokio::time::sleep(Duration::from_millis(500)).await;
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        let mut rx = h.subscribe();
        state(&mut rx, ModemState::Ready).await;
        tokio::time::pause();
        let b = beacon(&h).await;
        assert!(b.preferred, "the preference is kept");
        assert!(!b.active, "but not applied");
        assert_eq!(b.next_due_utc, None);
        advance(35 * MIN).await;
        assert_eq!(sent(&net), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn turning_beacons_off_stops_them_and_says_why() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, None)).await;
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        done(&h, AppCommand::SetBeacon { on: false }).await;
        let b = beacon(&h).await;
        assert!(!b.active && !b.preferred && b.next_due_utc.is_none());
        assert!(b.suppressed.is_some());
        advance(35 * MIN).await;
        assert_eq!(sent(&net), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn arming_needs_an_idle_attached_modem() {
        let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
        let why = conflict(h.call(AppCommand::SetBeacon { on: true }).await);
        assert!(why.contains("no station"), "{why}");
        assert!(conflict(h.call(AppCommand::SendBeaconNow).await).contains("no station"));
    }

    async fn activity_entries(h: &AppHandle) -> Vec<(String, String)> {
        match h.call(AppCommand::Activity).await.unwrap() {
            CommandOk::Activity { entries } => entries.into_iter().map(|e| (e.kind, e.text)).collect(),
            other => panic!("{other:?}"),
        }
    }

    /// The Broadcast button sends a general broadcast, `CALL: message`, not a
    /// beacon (owner, 26 Nov).
    #[tokio::test(flavor = "current_thread")]
    async fn a_broadcast_message_goes_out_as_a_general_broadcast() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, Some("KG33"))).await;
        tokio::time::pause();
        // Refused without sending: empty, too long, more than one line.
        for bad in ["", "   ", &"x".repeat(41), "two\nlines"] {
            let why = conflict(h.call(AppCommand::SendBroadcastMessage { text: bad.into() }).await);
            assert!(why.starts_with("not sent: "), "{bad:?}: {why}");
        }
        assert_eq!(sent(&net), 0);

        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        advance(Duration::from_secs(1)).await;
        advance(Duration::from_secs(20)).await;
        let before = beacon(&h).await;

        done(&h, AppCommand::SendBroadcastMessage { text: "  QRV 14.074, 73 de ZS6PS ".into() }).await;
        settle().await;
        advance(Duration::from_secs(1)).await;
        assert_eq!(sent(&net), 2);
        assert_eq!(net.broadcast_log()[1].1, b"ZS6ABC: QRV 14.074, 73 de ZS6PS");
        // It is not a beacon: the beacon's own record does not move.
        assert_eq!(beacon(&h).await.last_utc, before.last_utc);
        // And the message stays in the activity log, for the Broadcasts page.
        let entries = activity_entries(&h).await;
        let said: Vec<_> = entries.iter().filter(|(kind, _)| kind == "message").map(|(_, text)| text).collect();
        assert_eq!(said, ["Message sent: QRV 14.074, 73 de ZS6PS"], "{entries:?}");
        // One transmission is one row: no generic line is added beside it.
        assert!(!entries.iter().any(|(_, text)| text.starts_with("message transmitted")), "{entries:?}");
        assert_eq!(entries.iter().filter(|(_, text)| text.starts_with("beacon transmitted")).count(), 1, "{entries:?}");

        advance(Duration::from_secs(20)).await;
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        let log = net.broadcast_log();
        let (a, b) = (Beacon::decode(&log[0].1).unwrap(), Beacon::decode(&log[2].1).unwrap());
        assert_eq!(b.seq, a.seq.wrapping_add(1), "the message took no beacon sequence number");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn beacon_now_goes_through_the_gate_and_leaves_the_schedule_alone() {
        let net = FakeNet::start().await;
        net.set_unanswered_calls(true);
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), profile(&net, Some("KG33"))).await;
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        let before = beacon(&h).await;

        // While a call is being placed: refused by the same gate as a CQ.
        done(&h, AppCommand::Call { peer: "ZS6PEER".into() }).await;
        let why = conflict(h.call(AppCommand::SendBeaconNow).await);
        assert_eq!(why, "not sent: a call is being placed");
        assert_eq!(why, conflict(h.call(AppCommand::SendCq).await));
        assert_eq!(sent(&net), 0);
        settle().await;
        let mut outcomes = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Broadcast { kind: BroadcastKind::Beacon, outcome } = ev.event {
                outcomes.push(outcome);
            }
        }
        done(&h, AppCommand::CancelCall).await;
        state(&mut rx, ModemState::Listening).await;
        settle().await;

        // Idle: it goes, once, and the schedule is as it was.
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        advance(Duration::from_secs(1)).await;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Broadcast { kind: BroadcastKind::Beacon, outcome } = ev.event {
                outcomes.push(outcome);
            }
        }
        assert_eq!(sent(&net), 1);
        assert_eq!(
            outcomes,
            [BroadcastOutcome::Refused { reason: why }, BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]
        );
        let after = beacon(&h).await;
        assert!(after.active && after.preferred);
        assert_eq!(after.next_due_utc, before.next_due_utc, "the scheduled beacon did not move");
        assert!(after.last_utc.is_some());
        // Not armed: a one-time beacon still works.
        done(&h, AppCommand::SetBeacon { on: false }).await;
        advance(Duration::from_secs(20)).await;
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        assert_eq!(sent(&net), 2);
        assert!(!beacon(&h).await.active);
        // The sequence number went up by one.
        let log = net.broadcast_log();
        let (a, b) = (Beacon::decode(&log[0].1).unwrap(), Beacon::decode(&log[1].1).unwrap());
        assert_eq!(b.seq, a.seq.wrapping_add(1));
    }

    /// What a second client of Mercury's broadcast port gets (lab Q5, F5):
    /// the connection is made but nothing is read, and nothing is keyed.
    /// Once `release` is sent, it reads what is waiting, as Mercury does when
    /// the holder lets go.
    struct StalledPort {
        port: u16,
        /// Connections made to it.
        conns: Arc<AtomicUsize>,
        /// Bytes read after the release.
        read: Arc<AtomicUsize>,
        /// How each released connection ended (`eof`, or the error kind).
        ended: Arc<std::sync::Mutex<Vec<String>>>,
        release: tokio::sync::watch::Sender<bool>,
    }

    async fn stalled_broadcast_port() -> StalledPort {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (release, watch) = tokio::sync::watch::channel(false);
        let fake = StalledPort { port, conns: Arc::default(), read: Arc::default(), ended: Arc::default(), release };
        let (conns, read, ended) = (fake.conns.clone(), fake.read.clone(), fake.ended.clone());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                conns.fetch_add(1, Ordering::SeqCst);
                let (read, ended, mut watch) = (read.clone(), ended.clone(), watch.clone());
                tokio::spawn(async move {
                    let _ = watch.wait_for(|go| *go).await;
                    let mut buf = [0u8; 2048];
                    let end = loop {
                        match sock.read(&mut buf).await {
                            Ok(0) => break "eof".to_string(),
                            Ok(n) => {
                                read.fetch_add(n, Ordering::SeqCst);
                            }
                            Err(e) => break format!("{:?}", e.kind()),
                        }
                    };
                    ended.lock().unwrap().push(end);
                });
            }
        });
        fake
    }

    fn beacon_outcomes(rx: &mut Receiver<EventEnvelope>) -> Vec<BroadcastOutcome> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Broadcast { kind: BroadcastKind::Beacon, outcome } = ev.event {
                out.push(outcome);
            }
        }
        out
    }

    async fn activity_texts(h: &AppHandle) -> Vec<String> {
        match h.call(AppCommand::Activity).await.unwrap() {
            CommandOk::Activity { entries } => entries.into_iter().map(|e| e.text).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_silent_message_resets_the_port_and_leaves_beacons_alone() {
        let net = FakeNet::start().await;
        let fake = stalled_broadcast_port().await;
        let mut p = profile(&net, Some("KG33"));
        if let ModemTarget::Existing { broadcast_port, .. } = &mut p.modem {
            *broadcast_port = Some(fake.port);
        }
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), p).await;
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        let before = beacon(&h).await;
        for round in 1..=2 {
            done(&h, AppCommand::SendBroadcastMessage { text: "anyone there?".into() }).await;
            settle().await;
            advance(Duration::from_secs(16)).await;
            let mut outcomes = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                if let AppEvent::Broadcast { kind, outcome } = ev.event {
                    outcomes.push((kind, outcome));
                }
            }
            let silent = Some(NotConfirmedReason::BroadcastPortSilent);
            assert_eq!(
                outcomes,
                [
                    (BroadcastKind::Message, BroadcastOutcome::Submitted),
                    (BroadcastKind::Message, BroadcastOutcome::NotConfirmed { reason: silent })
                ]
            );
            // The unread bytes are discarded and a fresh connection is made.
            for _ in 0..50 {
                if fake.conns.load(Ordering::SeqCst) > round {
                    break;
                }
                settle().await;
            }
            assert_eq!(fake.conns.load(Ordering::SeqCst), round + 1);
        }
        let after = beacon(&h).await;
        assert!(after.active);
        assert_eq!((after.next_due_utc, after.suppressed), (before.next_due_utc, before.suppressed));
        let entries = activity_entries(&h).await;
        assert!(
            !entries.iter().any(|(kind, text)| kind == "message" || text.starts_with("beacons held")),
            "{entries:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_silent_broadcast_connection_is_reset_and_requires_rearming() {
        // Lab F3 and F5: another program holds Mercury's broadcast port.
        let net = FakeNet::start().await;
        let fake = stalled_broadcast_port().await;
        let mut p = profile(&net, Some("KG33"));
        if let ModemTarget::Existing { broadcast_port, .. } = &mut p.modem {
            *broadcast_port = Some(fake.port);
        }
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), p).await;
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        let silent = NotConfirmedReason::BroadcastPortSilent;
        let refused = BroadcastOutcome::Refused { reason: format!("not sent: {}", silent.text()) };
        assert_eq!(fake.conns.load(Ordering::SeqCst), 1);

        // Beacon now: written, never read, never keyed. A second one while
        // the first is unconfirmed is not written on the same connection.
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        let busy = conflict(h.call(AppCommand::SendBeaconNow).await);
        assert_eq!(busy, "not sent: a broadcast is still being sent");
        advance(Duration::from_secs(16)).await;
        assert_eq!(
            beacon_outcomes(&mut rx),
            [
                BroadcastOutcome::Submitted,
                BroadcastOutcome::Refused { reason: busy },
                BroadcastOutcome::NotConfirmed { reason: Some(silent) }
            ]
        );
        let log = activity_texts(&h).await;
        assert!(log.iter().any(|t| t.contains(silent.text())), "{log:?}");
        assert!(log.iter().any(|t| t.starts_with("broadcast port reset")), "{log:?}");
        let st = beacon(&h).await;
        assert!(st.active && st.next_due_utc.is_some(), "one is not enough to hold: {st:?}");

        // The connection is gone, and no new one is made by itself: the next
        // beacon is refused with the reason, the second in a row, so beacons hold.
        assert_eq!(conflict(h.call(AppCommand::SendBeaconNow).await), format!("not sent: {}", silent.text()));
        assert_eq!(beacon_outcomes(&mut rx), [refused]);
        let st = beacon(&h).await;
        assert!(st.active, "still armed: the operator's choice stands");
        assert_eq!(st.next_due_utc, None, "held, not silently re-armed");
        assert_eq!(st.suppressed.as_deref(), Some(silent.text()));
        assert!(activity_texts(&h).await.iter().any(|t| t.starts_with("beacons held")));
        advance(40 * MIN).await;
        assert!(beacon_outcomes(&mut rx).is_empty());
        assert_eq!(fake.conns.load(Ordering::SeqCst), 1, "no reconnect by itself");

        // Model the holder letting go. Winsock discards its unread bytes on
        // reset. Linux can deliver bytes buffered before the reset.
        fake.release.send_replace(true);
        for _ in 0..50 {
            if !fake.ended.lock().unwrap().is_empty() {
                break;
            }
            settle().await;
        }
        if cfg!(windows) {
            assert_eq!(fake.read.load(Ordering::SeqCst), 0, "a stale beacon would have gone out");
        }
        let ended = fake.ended.lock().unwrap().clone();
        assert_eq!(ended, ["ConnectionReset"], "the connection was reset, not closed in order");

        // Re-arming is the operator's retry: a fresh connection.
        done(&h, AppCommand::SetBeacon { on: false }).await;
        done(&h, AppCommand::SetBeacon { on: true }).await;
        for _ in 0..50 {
            if fake.conns.load(Ordering::SeqCst) == 2 {
                break;
            }
            settle().await;
        }
        assert_eq!(fake.conns.load(Ordering::SeqCst), 2, "re-arming reopens the broadcast port");
        let st = beacon(&h).await;
        assert!(st.active && st.next_due_utc.is_some() && st.suppressed.is_none(), "{st:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_beacon_the_modem_keyed_has_no_port_reason() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, mut rx) = ready(dir.path(), profile(&net, Some("KG33"))).await;
        tokio::time::pause();
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        // The fake keys for 100 ms: let that happen before the window ends.
        advance(Duration::from_secs(1)).await;
        advance(Duration::from_secs(15)).await;
        assert_eq!(beacon_outcomes(&mut rx), [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
        let log = activity_texts(&h).await;
        assert!(!log.iter().any(|t| t.contains("another program")), "{log:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn beacon_without_grid_when_include_grid_is_off() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = ready(dir.path(), profile(&net, Some("KG33xx"))).await;
        tokio::time::pause();
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        done(&h, AppCommand::SetBeaconContent { include_grid: false }).await;
        assert!(!beacon(&h).await.include_grid);
        advance(Duration::from_secs(20)).await;
        done(&h, AppCommand::SendBeaconNow).await;
        settle().await;
        let log = net.broadcast_log();
        assert_eq!(log.len(), 2);
        assert_eq!(Beacon::decode(&log[0].1).unwrap().grid.as_deref(), Some("KG33xx"));
        let without = Beacon::decode(&log[1].1).unwrap();
        assert_eq!((without.grid, without.callsign.as_str(), without.caps), (None, "ZS6ABC", CAP_CHAT));
        // A profile with no grid sends none even when asked to include it.
        done(&h, AppCommand::SetBeaconContent { include_grid: true }).await;
        assert!(beacon(&h).await.include_grid);
    }
}

#[cfg(feature = "sim")]
mod refused {
    use super::*;
    use mc_core::app::{
        spawn_app, AppCommand, AppEvent, AppOptions, BroadcastKind, BroadcastOutcome, CommandOk, ModemState,
    };
    use mc_core::profile::{ModemTarget, StationProfile};
    use mc_storage::Store;
    use mercury_adapter::fake::FakeNet;

    async fn settle() {
        for _ in 0..40 {
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A beacon the modem task refuses (no broadcast port) is not "last sent",
    /// and does not use up a sequence number.
    #[tokio::test(flavor = "current_thread")]
    async fn refused_beacon_does_not_advance_last_utc() {
        let net = FakeNet::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut p = StationProfile::new_existing("ZS6ABC");
        p.modem = ModemTarget::Existing {
            host: "127.0.0.1".into(),
            control_port: net.a_base,
            data_port: net.a_base + 1,
            broadcast_port: None,
            telemetry_url: None,
        };
        let h = spawn_app(
            Store::open_in_memory().unwrap(),
            AppOptions {
                sim_root: Some(dir.path().to_path_buf()),
                stations_root: Some(dir.path().join("stations")),
                ..AppOptions::default()
            },
        );
        let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
            CommandOk::Profile(p) => p.id,
            other => panic!("{other:?}"),
        };
        let mut rx = h.subscribe();
        h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
        loop {
            if let AppEvent::ModemState { state: ModemState::Ready, .. } = rx.recv().await.unwrap().event {
                break;
            }
        }
        tokio::time::pause();
        h.call(AppCommand::SetBeacon { on: true }).await.unwrap();
        h.call(AppCommand::SendBeaconNow).await.unwrap(); // the actor's gate cannot see the port
        settle().await;
        for _ in 0..25 {
            tokio::time::advance(Duration::from_secs(30)).await;
            settle().await;
        }
        let mut refused = 0;
        while let Ok(ev) = rx.try_recv() {
            match ev.event {
                AppEvent::Broadcast { kind: BroadcastKind::Beacon, outcome: BroadcastOutcome::Refused { .. } } => {
                    refused += 1
                }
                AppEvent::Broadcast { kind: BroadcastKind::Beacon, outcome: BroadcastOutcome::Submitted } => {
                    panic!("nothing was submitted")
                }
                _ => {}
            }
        }
        assert!(refused >= 2, "the one-time and a scheduled beacon were refused: {refused}");
        match h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => assert_eq!(s.beacon.last_utc, None, "no beacon went out"),
            other => panic!("{other:?}"),
        }
    }
}

/// M3c Task 11 (B7): the extended beacon (MCB/1 kind 2) and its heard fields.
mod extended {
    use mc_core::app::{beacon_payload, bytes_left, BeaconPrefs};
    use mc_core::profile::StationProfile;
    use mch_protocol::beacon::{self as mcb, listen_capacity, Received, Tlv};

    const DATAC3: u8 = 1;

    fn station() -> StationProfile {
        let mut p = StationProfile::new_existing("ZS6ABC");
        p.grid = Some("KG33xx".into());
        p.name = Some("Pat".into());
        p.qth = Some("Pretoria".into());
        p
    }

    fn ticked(name: bool, qth: bool, status: Option<&str>) -> BeaconPrefs {
        BeaconPrefs { name, qth, status: status.map(String::from), ..BeaconPrefs::default() }
    }

    #[test]
    fn nothing_ticked_sends_kind1() {
        let payload = beacon_payload(&BeaconPrefs::default(), &station(), 9, DATAC3).unwrap();
        assert!(matches!(mcb::classify(&payload), Received::Beacon(_)), "{payload:?}");
    }

    #[test]
    fn bytes_left_matches_encoded_len() {
        for prefs in [ticked(false, false, None), ticked(true, false, None), ticked(true, true, Some("QRV 40m"))] {
            let len = beacon_payload(&prefs, &station(), 1, DATAC3).unwrap().len() as i32;
            assert_eq!(bytes_left(&prefs, &station(), DATAC3), listen_capacity(DATAC3).unwrap() as i32 - len);
        }
        assert!(bytes_left(&ticked(true, true, Some("QRV 40m")), &station(), 7) < 0, "over DATAC15's 27 bytes");
    }

    #[test]
    fn long_profile_qth_left_out_of_beacon() {
        let mut p = station();
        // A profile QTH may hold 48 bytes; the beacon's TLV holds 24.
        p.qth = Some("Q".repeat(25));
        let payload = beacon_payload(&ticked(true, true, None), &p, 1, DATAC3).unwrap();
        let Received::Extended(e) = mcb::classify(&payload) else { panic!("not kind 2") };
        assert_eq!(e.tlvs, vec![Tlv::Name("Pat".into())], "left out, not truncated");
    }

    #[cfg(feature = "sim")]
    mod service {
        use super::*;
        use mc_core::app::{spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, CommandOk, ModemState};
        use mc_core::profile::ModemTarget;
        use mc_storage::Store;
        use mch_protocol::beacon::{Beacon, ExtendedBeacon, CAP_CHAT};
        use mercury_adapter::fake::FakeNet;
        use mercury_adapter::fake_ws::FakeWs;
        use std::path::Path;
        use std::time::Duration;
        use tokio::sync::broadcast::Receiver;
        use tokio::time::Instant;

        fn status_on_mode(listen_mode: u8) -> String {
            format!(
                r#"{{"type":"status","bitrate":0,"snr":0.0,"direction":"rx","tx_gain_db":0.0,"audio_ok":true,"audio_error":"","arq_tx_mode":"","arq_rx_mode":"","radio_frequency_hz":null,"radio_frequency_age_ms":null,"freq_offset_hz":0.0,"freq_offset_age_ms":0,"listen_mode":{listen_mode},"busy":""}}"#
            )
        }

        async fn app(
            root: &Path,
            net: &FakeNet,
            ws: Option<&FakeWs>,
        ) -> (AppHandle, Receiver<mc_core::app::EventEnvelope>) {
            let mut p = station();
            p.modem = ModemTarget::Existing {
                host: "127.0.0.1".into(),
                control_port: net.a_base,
                data_port: net.a_base + 1,
                broadcast_port: Some(net.a_bcast),
                telemetry_url: ws.map(|w| w.url.clone()),
            };
            let opts = AppOptions {
                sim_root: Some(root.into()),
                stations_root: Some(root.join("stations")),
                ..AppOptions::default()
            };
            let h = spawn_app(Store::open_in_memory().unwrap(), opts);
            let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
                CommandOk::Profile(p) => p.id,
                other => panic!("{other:?}"),
            };
            let mut rx = h.subscribe();
            h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("listening").unwrap();
                if let AppEvent::ModemState { state: ModemState::Listening, .. } = ev.event {
                    break;
                }
            }
            (h, rx)
        }

        fn content(name: bool, qth: bool, status: Option<&str>) -> AppCommand {
            AppCommand::SetBeaconFields { name, qth, status: status.map(String::from) }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn ticked_fields_send_kind2() {
            let dir = tempfile::tempdir().unwrap();
            let net = FakeNet::start().await;
            let (h, _rx) = app(dir.path(), &net, None).await;
            h.call(content(true, true, Some("QRV 40m"))).await.unwrap();
            h.call(AppCommand::SetBeacon { on: true }).await.unwrap();
            h.call(AppCommand::SendBeaconNow).await.unwrap();
            let end = Instant::now() + Duration::from_secs(10);
            while net.broadcast_log().is_empty() {
                assert!(Instant::now() < end, "no beacon went out");
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let (_, payload) = net.broadcast_log().remove(0);
            let Received::Extended(e) = mcb::classify(&payload) else { panic!("not kind 2: {payload:?}") };
            let want = vec![Tlv::Name("Pat".into()), Tlv::Qth("Pretoria".into()), Tlv::Status("QRV 40m".into())];
            assert_eq!(e.tlvs, want);
            assert_eq!(e.beacon.callsign, "ZS6ABC");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn selection_over_capacity_not_saved() {
            let dir = tempfile::tempdir().unwrap();
            let net = FakeNet::start().await;
            let ws = FakeWs::start().await;
            ws.set_status(Some(&status_on_mode(7))); // DATAC15: 27 bytes
            let (h, _rx) = app(dir.path(), &net, Some(&ws)).await;
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let s = match h.call(AppCommand::Snapshot).await.unwrap() {
                    CommandOk::Snapshot(s) => s,
                    other => panic!("{other:?}"),
                };
                if s.telemetry.listen_mode == Some(7) {
                    break;
                }
                assert!(Instant::now() < end, "no DATAC15 from telemetry");
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let e = h.call(content(true, true, Some("QRV 40m"))).await.unwrap_err();
            assert!(matches!(e, AppError::Validation { .. }), "{e:?}");
            match h.call(AppCommand::BeaconContent).await.unwrap() {
                CommandOk::BeaconContent(c) => {
                    assert_eq!((c.name, c.qth, c.status), (false, false, None), "not saved");
                    assert!(c.bytes_left >= 0);
                }
                other => panic!("{other:?}"),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_message_over_the_modes_capacity_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let net = FakeNet::start().await;
            let ws = FakeWs::start().await;
            ws.set_status(Some(&status_on_mode(7))); // DATAC15: 27 bytes
            let (h, _rx) = app(dir.path(), &net, Some(&ws)).await;
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let CommandOk::Snapshot(s) = h.call(AppCommand::Snapshot).await.unwrap() else { panic!("snapshot") };
                if s.telemetry.listen_mode == Some(7) {
                    break;
                }
                assert!(Instant::now() < end, "no DATAC15 from telemetry");
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            // "ZS6ABC: " is 8 bytes, so 19 more fit and 20 do not.
            let e = h.call(AppCommand::SendBroadcastMessage { text: "x".repeat(20) }).await.unwrap_err();
            let AppError::Conflict { message } = e else { panic!("{e:?}") };
            assert_eq!(
                message,
                "not sent: with your callsign it is 28 bytes, over the 27-byte limit of this listen mode"
            );
            assert!(net.broadcast_log().is_empty());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn kind2_fills_heard_name_qth_status() {
            let dir = tempfile::tempdir().unwrap();
            let net = FakeNet::start().await;
            let (_h, mut rx) = app(dir.path(), &net, None).await;
            let b = Beacon::new(3, "ZS1XYZ", Some("JF96"), CAP_CHAT).unwrap();
            let tlvs = vec![Tlv::Name("Sam".into()), Tlv::Qth("Cape Town".into()), Tlv::Status("QRV 20m".into())];
            assert!(net.air_broadcast(1, ExtendedBeacon::new(b, tlvs).unwrap().encode()));
            let end = Instant::now() + Duration::from_secs(10);
            let station = loop {
                let ev = tokio::time::timeout_at(end, rx.recv()).await.expect("heard").unwrap();
                if let AppEvent::Heard { station } = ev.event {
                    if station.callsign == "ZS1XYZ" {
                        break station;
                    }
                }
            };
            let field =
                |r: &Option<mc_core::app::Reading<String>>| r.as_ref().map(|r| (r.value.clone(), r.source.clone()));
            assert_eq!(field(&station.name), Some(("Sam".into(), "beacon".into())));
            assert_eq!(field(&station.qth), Some(("Cape Town".into(), "beacon".into())));
            assert_eq!(field(&station.status), Some(("QRV 20m".into(), "beacon".into())));
            assert_eq!(station.grid.as_deref(), Some("JF96"));
        }
    }
}
