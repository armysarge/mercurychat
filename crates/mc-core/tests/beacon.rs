//! M3a Task 6: periodic beacons (DSC-03): opt-in, jittered, idle-only, never
//! bursting, never armed by a restart or a recovery.
//!
//! `scheduler` drives the pure state machine with synthetic times. `service`
//! runs the app against the FakeNet modem on tokio's paused clock: setup is in
//! real time, then the clock is paused and moved by hand.

use mc_core::app::{BeaconScheduler, ModemState, Tick, BUSY_HOLD};
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
        tokio::time::pause();
        done(&h, AppCommand::SetBeacon { on: true }).await;
        assert!(beacon(&h).await.active);
        net.kill_client(0); // Mercury closes our sockets
        state(&mut rx, ModemState::Recovering).await;
        let b = beacon(&h).await;
        assert!(!b.active, "recovery disarms");
        assert!(b.preferred, "but the preference stays");
        state(&mut rx, ModemState::Ready).await;
        let b = beacon(&h).await;
        assert!(!b.active && b.preferred && b.next_due_utc.is_none(), "{b:?}");
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
        state(&mut rx, ModemState::Ready).await;
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
