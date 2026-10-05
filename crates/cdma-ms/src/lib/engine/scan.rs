use crate::forward_rx::{PilotMeasurement, SyncParameters};
use crate::ms::MsEvent;
use crate::prl_scan::{PrlVerdict, ScanChannel};

use super::{ChannelResult, Engine, ScanConfig, ScanMode, ScanPhase, ScanState, TuneRequest};

impl Engine {
    pub fn set_scan(&mut self, scan: Option<ScanConfig>) {
        if self.scanning() {
            log::info!("ms_scan: scan replaced while running");
        }
        self.scan = scan.map(ScanState::new);
    }

    pub fn pending_tune(&mut self) -> Option<TuneRequest> {
        let scan = self.scan.as_mut()?;
        let ScanPhase::Tune(index) = scan.phase else {
            return None;
        };
        scan.phase = ScanPhase::AwaitTune(index);
        Some(TuneRequest {
            index,
            total: scan.cfg.plan.channels.len(),
            channel: scan.cfg.plan.channels[index].clone(),
        })
    }

    pub fn on_tuned(&mut self, request: &TuneRequest, result: Result<(), String>) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        if scan.phase != ScanPhase::AwaitTune(request.index) {
            return;
        }
        match result {
            Ok(()) => {
                self.forward.reset();
                self.pilot_locked_at = None;
                self.sync_at = None;
                self.overhead_at = None;
                self.sync_warned = false;
                self.overhead_warned = false;
                scan.pilot_report_pending = false;
                scan.results.push(ChannelResult {
                    channel: request.channel.clone(),
                    pilot: None,
                    measurement: PilotMeasurement::default(),
                    sync: None,
                    verdict: None,
                });
                scan.phase = ScanPhase::Dwell {
                    index: request.index,
                    fed: 0,
                    pilot_at: None,
                };
                log::info!(
                    "ms_scan: {} tuned, searching (dwell={}ms)",
                    request.tag(),
                    scan.cfg.dwell_ms
                );
                self.last_tuned_reverse_hz = Some(request.channel.reverse_hz());
                let ch = &request.channel;
                self.core.emit(MsEvent::ChannelTuned {
                    band_class: ch.band_class.as_str().to_string(),
                    channel: ch.channel,
                    frequency_hz: ch.frequency_hz as u64,
                    index: request.index as u32,
                    total: request.total as u32,
                });
                self.core.begin_pilot_search();
            }
            Err(e) => {
                log::warn!("ms_scan: {} tune failed: {}", request.tag(), e);
                self.next_channel(request.index + 1);
            }
        }
    }

    pub fn scanning(&self) -> bool {
        self.scan
            .as_ref()
            .map(|s| !matches!(s.phase, ScanPhase::Idle | ScanPhase::Finished))
            .unwrap_or(false)
    }

    pub(super) fn current_channel(&self) -> Option<(usize, &ScanChannel)> {
        let scan = self.scan.as_ref()?;
        let index = match scan.phase {
            ScanPhase::Dwell { index, .. } | ScanPhase::AwaitTune(index) => index,
            ScanPhase::Finished => scan.camped_index?,
            _ => return None,
        };
        Some((index, &scan.cfg.plan.channels[index]))
    }

    pub(super) fn channel_tag(&self) -> String {
        match self.current_channel() {
            Some((index, ch)) => format!(
                "[{:>2}/{}] {}",
                index + 1,
                self.scan
                    .as_ref()
                    .map(|s| s.cfg.plan.channels.len())
                    .unwrap_or(0),
                ch.label()
            ),
            None => String::new(),
        }
    }

    pub(super) fn emit_pilot_search(&mut self, found: bool, m: &PilotMeasurement) {
        let Some((_, ch)) = self.current_channel() else {
            return;
        };
        let (band_class, channel) = (ch.band_class.as_str().to_string(), ch.channel);
        self.core.emit(MsEvent::PilotSearch {
            band_class,
            channel,
            found,
            ec_io_db: (!m.ec_io_db.is_nan()).then_some(m.ec_io_db),
            rx_power_dbfs: m.rx_power_dbfs,
        });
    }

    pub(super) fn check_scan_timers(&mut self) {
        let Some(scan) = &self.scan else {
            return;
        };
        let ScanPhase::Dwell {
            index,
            fed,
            pilot_at,
        } = scan.phase
        else {
            return;
        };
        let dwell = self.samples_for_ms(scan.cfg.dwell_ms);
        let pilot_timeout = self.samples_for_ms(scan.cfg.pilot_timeout_ms);
        let sync_timeout = self.samples_for_ms(scan.cfg.sync_timeout_ms);
        match pilot_at {
            None if fed >= dwell => {
                let m = self.forward.measurements().snapshot();
                log::info!(
                    "ms_scan: {} no pilot after {}ms (rx {:.1} dBFS)",
                    self.channel_tag(),
                    self.air_ms(fed),
                    m.rx_power_dbfs,
                );
                if let Some(last) = self.scan.as_mut().and_then(|s| s.results.last_mut()) {
                    last.pilot = Some(false);
                    last.measurement = m.clone();
                }
                self.emit_pilot_search(false, &m);
                self.core.on_system_rejected();
                self.next_channel(index + 1);
            }
            Some(at) if fed.saturating_sub(at) >= sync_timeout || fed >= pilot_timeout => {
                self.flush_pilot_report(true);
                log::warn!(
                    "ms_scan: {} pilot locked but no sync within {}ms, moving on",
                    self.channel_tag(),
                    self.air_ms(fed.saturating_sub(at))
                );
                self.core.on_system_rejected();
                self.next_channel(index + 1);
            }
            _ => {}
        }
    }

    pub(super) fn next_channel(&mut self, next: usize) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        if next >= scan.cfg.plan.channels.len() {
            scan.phase = ScanPhase::Finished;
            let result = match scan.cfg.mode {
                ScanMode::Camp => "no_permitted_system",
                ScanMode::Survey => "survey",
            };
            self.finish_scan(result);
        } else {
            scan.phase = ScanPhase::Tune(next);
        }
    }

    pub(super) fn finish_scan(&mut self, result: &str) {
        let camped_channel = (result == "camped")
            .then(|| {
                self.current_channel()
                    .map(|(index, ch)| (index, ch.reverse_hz()))
            })
            .flatten();
        let Some(scan) = &mut self.scan else {
            return;
        };
        scan.phase = ScanPhase::Finished;
        scan.result = Some(result.to_string());
        scan.camped_index = camped_channel.map(|(index, _)| index);
        if let Some((_, hz)) = camped_channel {
            self.pending_reverse_hz = Some(hz);
        }
        let elapsed_ms = (scan.fed_total as f64 * 1000.0 / self.sample_rate_hz) as u64;
        let (channels, pilots, syncs, rejected) = (
            scan.cfg.plan.channels.len() as u32,
            scan.pilots,
            scan.syncs,
            scan.rejected,
        );
        log::info!(
            "ms_scan: done result={} elapsed={:.1}s | channels={} pilots={} syncs={} rejected={}",
            result,
            elapsed_ms as f64 / 1000.0,
            channels,
            pilots,
            syncs,
            rejected
        );
        self.core.emit(MsEvent::ScanFinished {
            result: result.to_string(),
            elapsed_ms,
            channels,
            pilots,
            syncs,
        });
    }

    pub(super) fn apply_scan_sync(&mut self, params: SyncParameters) {
        self.flush_pilot_report(true);
        let (pilot_pn, sid, nid, p_rev, sys_time) = (
            params.pilot_pn,
            params.sid,
            params.nid,
            params.p_rev,
            params.sys_time,
        );
        let tag = self.channel_tag();
        let Some(scan) = &mut self.scan else {
            return;
        };
        let ScanPhase::Dwell { index, fed, .. } = scan.phase else {
            return;
        };
        scan.syncs += 1;
        let verdict = scan.cfg.plan.verdict(sid, nid);
        let mode = scan.cfg.mode;
        if let Some(last) = scan.results.last_mut() {
            last.sync = Some(params.clone());
            last.verdict = Some(verdict.clone());
        }
        let at_ms = fed as f64 * 1000.0 / self.sample_rate_hz / 1000.0;
        log::info!(
            "ms_scan: {} sync decoded pilot_pn={} sid={} nid={} p_rev={} sys_time={} t={:.2}s",
            tag,
            pilot_pn,
            sid,
            nid,
            p_rev,
            sys_time,
            at_ms
        );
        let (record, roaming_indicator, geo) = match &verdict {
            PrlVerdict::Permitted {
                record,
                roaming_indicator,
                geo,
            } => (*record, *roaming_indicator, *geo),
            PrlVerdict::Negative { record } => (Some(*record), None, None),
            PrlVerdict::Unlisted => (None, None, None),
        };
        self.core.emit(MsEvent::PrlVerdict {
            sid,
            nid,
            permitted: verdict.permitted(),
            reason: verdict.reason().to_string(),
            record: record.map(|r| r as u32),
            roaming_indicator,
        });
        if mode == ScanMode::Survey {
            log::info!(
                "ms_scan: {} verdict={} reason={} record={:?} geo={:?} roam_ind={:?} (survey, not camping)",
                tag,
                if verdict.permitted() {
                    "permitted"
                } else {
                    "not_permitted"
                },
                verdict.reason(),
                record,
                geo,
                roaming_indicator
            );
            self.core.emit(MsEvent::SyncDecoded(params));
            self.core.on_system_rejected();
            self.next_channel(index + 1);
        } else if verdict.permitted() {
            log::info!(
                "ms_scan: {} verdict=permitted reason={} record={:?} geo={:?} roam_ind={:?} -> camp",
                tag,
                verdict.reason(),
                record,
                geo,
                roaming_indicator
            );
            self.core.on_pilot_acquired(pilot_pn as u32);
            self.core.on_sync_decoded(params);
            self.core.begin_idle_timers(self.system_time_chips());
            self.finish_scan("camped");
        } else {
            let scan = self.scan.as_mut().expect("scan state");
            scan.rejected += 1;
            log::info!(
                "ms_scan: {} verdict=not_permitted reason={} record={:?} sid={} nid={}",
                tag,
                verdict.reason(),
                record,
                sid,
                nid
            );
            self.core.on_system_rejected();
            self.next_channel(index + 1);
        }
    }
}
