//! The actor's side of information requests (M3c B6): answering the peer
//! automatically from what the operator shares, asking on the operator's
//! command, and filling the QSO card's empty fields from the peer's report.

use super::{storage, App};
use crate::app::contract::{AppError, CommandOk};
use crate::app::events::AppEvent;
use crate::app::info::{
    rst_from_snr, snr_report, station_share, InfoLimiter, PeerSnr, PeerStation, ShareConfig, PEER_REPORT,
};
use crate::app::logbook::Qso;
use crate::app::modem::ModemCmd;
use crate::session::PeerMsg;
use mch_protocol::payload::{InfoRequest, InfoResponse, InfoWhat, StationShare};
use mch_protocol::{ObjectId, CAP_INFO};
use std::collections::HashSet;
use tokio::time::Instant;

/// Per-session state: the answer allowance and the requests awaiting an answer.
#[derive(Debug, Default)]
pub(in crate::app) struct InfoSide {
    limiter: InfoLimiter,
    asked: HashSet<ObjectId>,
}

fn refused(reason: &'static str, message: &str) -> AppError {
    AppError::Refused { reason, message: message.into() }
}

impl App {
    /// A new session: a fresh allowance, and nothing asked yet.
    pub(super) fn info_session_started(&mut self) {
        self.info = InfoSide::default();
    }

    /// The session's peer, if it negotiated `info/1`; otherwise the refusal.
    fn info_peer(&self) -> Result<String, AppError> {
        let s = self.session.as_ref().ok_or_else(|| refused("info.refused.noSession", "there is no session"))?;
        if !s.caps.iter().any(|c| c == CAP_INFO) {
            return Err(refused("info.refused.unsupported", "not supported by this station"));
        }
        Ok(s.peer.clone())
    }

    fn send_peer(&self, m: PeerMsg) -> Result<(), AppError> {
        self.command_modem(ModemCmd::Peer(m))
    }

    pub(super) fn request_info(&mut self, what: InfoWhat) -> Result<CommandOk, AppError> {
        self.info_peer()?;
        let id = ObjectId::random();
        self.send_peer(PeerMsg::InfoRequest(id, InfoRequest { what }))?;
        self.info.asked.insert(id);
        Ok(CommandOk::Done)
    }

    pub(super) fn send_email(&mut self) -> Result<CommandOk, AppError> {
        self.info_peer()?;
        let email = self
            .active_profile()
            .and_then(|p| p.email)
            .filter(|e| !e.is_empty() && e.len() <= 254)
            .ok_or_else(|| refused("info.refused.noEmail", "no email is set for this station"))?;
        let share = StationShare { email: Some(email), ..StationShare::default() };
        let answer = InfoResponse { snr: None, station: Some(share), reason: None };
        self.send_peer(PeerMsg::InfoResponse(ObjectId::ZERO, Box::new(answer)))?;
        Ok(CommandOk::Done)
    }

    pub(super) fn share_settings(&self) -> Result<CommandOk, AppError> {
        let id = self.active_profile().map(|p| p.id).ok_or_else(|| super::conflict("no station is selected".into()))?;
        Ok(CommandOk::Share(ShareConfig::load(&self.store, &id)))
    }

    pub(super) fn set_share_settings(&mut self, mut config: ShareConfig) -> Result<CommandOk, AppError> {
        let id = self.active_profile().map(|p| p.id).ok_or_else(|| super::conflict("no station is selected".into()))?;
        let mut seen = HashSet::new();
        config.fields.retain(|f| seen.insert(*f));
        config.save(&mut self.store, &id).map_err(storage)?;
        Ok(CommandOk::Share(config))
    }

    /// A minor-1 message from the session's peer.
    pub(super) fn on_peer(&mut self, m: PeerMsg) {
        match m {
            PeerMsg::InfoRequest(id, req) => self.answer_info(id, req.what),
            PeerMsg::InfoResponse(id, r) => self.on_info_response(id, *r),
            m @ (PeerMsg::QsyPropose(..) | PeerMsg::QsyAnswer(..)) => self.on_peer_qsy(m),
        }
    }

    /// Answer automatically, at most once per kind every 10 s.
    fn answer_info(&mut self, id: ObjectId, what: InfoWhat) {
        let now = Instant::now();
        let empty = |reason: &str| InfoResponse { snr: None, station: None, reason: Some(reason.into()) };
        let answer = if !self.info.limiter.allow(what, now) {
            empty("rate")
        } else {
            match what {
                InfoWhat::Snr => match snr_report(self.telemetry.view(now).snr_db.as_ref(), &mc_storage::now_utc()) {
                    Ok(r) => InfoResponse { snr: Some(r), station: None, reason: None },
                    Err(reason) => empty(reason),
                },
                InfoWhat::Station => {
                    let share = match self.active_profile() {
                        Some(p) => station_share(&p, &ShareConfig::load(&self.store, &p.id)),
                        None => StationShare::default(),
                    };
                    InfoResponse { snr: None, station: Some(share), reason: None }
                }
            }
        };
        if let Err(e) = self.send_peer(PeerMsg::InfoResponse(id, Box::new(answer))) {
            tracing::warn!("could not answer the peer's {what:?} request: {e}");
        }
    }

    /// An answer to one of our requests, or the peer's unsolicited share;
    /// an answer to anything else is ignored.
    fn on_info_response(&mut self, id: ObjectId, r: InfoResponse) {
        if id != ObjectId::ZERO && !self.info.asked.remove(&id) {
            tracing::debug!("ignoring an information answer to a request we did not make");
            return;
        }
        let Some(peer) = self.session.as_ref().map(|s| s.peer.clone()) else { return };
        // Unsolicited, it is the email share and nothing else (M4 B6): an SNR or
        // station field nobody asked for fills nothing.
        let r = if id == ObjectId::ZERO {
            let email = r.station.and_then(|s| s.email);
            InfoResponse {
                snr: None,
                station: email.map(|email| StationShare { email: Some(email), ..StationShare::default() }),
                reason: None,
            }
        } else {
            r
        };
        self.fill_qso_from_peer(&r);
        self.emit(AppEvent::PeerInfo {
            peer,
            snr: r.snr.map(PeerSnr::from),
            station: r.station.map(PeerStation::from),
            reason: r.reason,
        });
    }

    /// The session's draft takes the peer's SNR as RST received and its name
    /// and QTH, only where the field is empty (B6).
    fn fill_qso_from_peer(&mut self, r: &InfoResponse) {
        let Some(id) = self.qso.as_ref().map(|q| q.id.clone()) else { return };
        let rst = r.snr.as_ref().map(|s| rst_from_snr(s.db));
        let station = r.station.as_ref();
        let (name, qth) = (station.and_then(|s| s.name.as_deref()), station.and_then(|s| s.qth.as_deref()));
        if rst.is_none() && name.is_none() && qth.is_none() {
            return;
        }
        let Some(store) = self.chat.as_mut() else { return };
        match store.fill_qso_from_peer(&id, rst.as_deref(), name, qth, PEER_REPORT) {
            Ok(row) => {
                self.qso = Some(row.clone());
                self.emit(AppEvent::Qso { qso: Qso::from(&row) });
            }
            Err(e) => tracing::warn!("could not fill the QSO record from the peer's report: {e}"),
        }
    }
}
