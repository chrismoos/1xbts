//! HRPD Route Update Protocol (Default). C.S0024-400 §6.6.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PilotId(pub u16);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteUpdateMessage {
    pub pilot_strengths: Vec<(PilotId, i8)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteUpdateState {
    pub active: Vec<PilotId>,
    pub candidate: Vec<PilotId>,
    pub neighbor: Vec<PilotId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundRouteEvent {
    ActivePilotChanged {
        from: Option<PilotId>,
        to: Option<PilotId>,
    },
    CandidateAdded(PilotId),
    NeighborAdded(PilotId),
}

pub const ACTIVE_THRESHOLD_DB: i8 = -12;
pub const NEIGHBOR_FLOOR_DB: i8 = -18;

impl RouteUpdateState {
    /// Apply a Route Update message to this state, returning the next state
    /// and the outbound events the transition produced.
    pub fn apply(&self, msg: &RouteUpdateMessage) -> (RouteUpdateState, Vec<OutboundRouteEvent>) {
        let prev = self;
        let mut next = RouteUpdateState::default();
        let mut events = Vec::new();

        let strongest = msg
            .pilot_strengths
            .iter()
            .filter(|(_, s)| *s >= ACTIVE_THRESHOLD_DB)
            .max_by_key(|(_, s)| *s)
            .map(|(p, _)| *p);

        if let Some(p) = strongest {
            next.active.push(p);
        }

        for (p, s) in &msg.pilot_strengths {
            if Some(*p) == strongest {
                continue;
            }
            if *s >= ACTIVE_THRESHOLD_DB {
                if !next.candidate.contains(p) {
                    next.candidate.push(*p);
                    if !prev.candidate.contains(p) {
                        events.push(OutboundRouteEvent::CandidateAdded(*p));
                    }
                }
            } else if *s >= NEIGHBOR_FLOOR_DB {
                if !next.neighbor.contains(p) {
                    next.neighbor.push(*p);
                    if !prev.neighbor.contains(p) {
                        events.push(OutboundRouteEvent::NeighborAdded(*p));
                    }
                }
            }
        }

        let prev_active = prev.active.first().copied();
        if prev_active != strongest {
            events.insert(
                0,
                OutboundRouteEvent::ActivePilotChanged {
                    from: prev_active,
                    to: strongest,
                },
            );
        }

        (next, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(pairs: &[(u16, i8)]) -> RouteUpdateMessage {
        RouteUpdateMessage {
            pilot_strengths: pairs.iter().map(|(p, s)| (PilotId(*p), *s)).collect(),
        }
    }

    #[test]
    fn empty_input_produces_empty_state() {
        let (s, e) = RouteUpdateState::default().apply(&msg(&[]));
        assert!(s.active.is_empty() && s.candidate.is_empty() && s.neighbor.is_empty());
        assert!(e.is_empty());
    }

    #[test]
    fn first_strong_promotes_to_active() {
        let (s, e) = RouteUpdateState::default().apply(&msg(&[(1, -5)]));
        assert_eq!(s.active, vec![PilotId(1)]);
        assert_eq!(
            e[0],
            OutboundRouteEvent::ActivePilotChanged {
                from: None,
                to: Some(PilotId(1))
            }
        );
    }

    #[test]
    fn strongest_wins_active_slot() {
        let (s, _) = RouteUpdateState::default().apply(&msg(&[(1, -8), (2, -3), (3, -10)]));
        assert_eq!(s.active, vec![PilotId(2)]);
        assert!(s.candidate.contains(&PilotId(1)));
        assert!(s.candidate.contains(&PilotId(3)));
    }

    #[test]
    fn neighbor_classification() {
        let (s, _) = RouteUpdateState::default().apply(&msg(&[(1, -5), (2, -15)]));
        assert_eq!(s.active, vec![PilotId(1)]);
        assert_eq!(s.neighbor, vec![PilotId(2)]);
    }

    #[test]
    fn sub_floor_pilots_dropped() {
        let (s, _) = RouteUpdateState::default().apply(&msg(&[(1, -5), (2, -25)]));
        assert!(!s.neighbor.contains(&PilotId(2)));
        assert!(!s.candidate.contains(&PilotId(2)));
    }

    #[test]
    fn active_handoff_emits_change_event() {
        let prev = RouteUpdateState {
            active: vec![PilotId(1)],
            ..Default::default()
        };
        let (s, e) = prev.apply(&msg(&[(1, -10), (2, -3)]));
        assert_eq!(s.active, vec![PilotId(2)]);
        assert!(matches!(
            e[0],
            OutboundRouteEvent::ActivePilotChanged {
                from: Some(PilotId(1)),
                to: Some(PilotId(2))
            }
        ));
    }

    #[test]
    fn stable_active_emits_no_change_event() {
        let prev = RouteUpdateState {
            active: vec![PilotId(1)],
            ..Default::default()
        };
        let (s, e) = prev.apply(&msg(&[(1, -5)]));
        assert_eq!(s.active, vec![PilotId(1)]);
        assert!(
            !e.iter()
                .any(|ev| matches!(ev, OutboundRouteEvent::ActivePilotChanged { .. }))
        );
    }

    #[test]
    fn repeated_candidate_not_re_emitted() {
        let prev = RouteUpdateState {
            active: vec![PilotId(1)],
            candidate: vec![PilotId(2)],
            ..Default::default()
        };
        let (_, e) = prev.apply(&msg(&[(1, -5), (2, -8)]));
        assert!(
            !e.iter()
                .any(|ev| matches!(ev, OutboundRouteEvent::CandidateAdded(_)))
        );
    }

    #[test]
    fn below_active_threshold_only_no_active() {
        let (s, e) = RouteUpdateState::default().apply(&msg(&[(1, -15), (2, -16)]));
        assert!(s.active.is_empty());
        assert_eq!(s.neighbor.len(), 2);
        assert!(
            matches!(
                e[0],
                OutboundRouteEvent::ActivePilotChanged {
                    from: None,
                    to: None
                }
            ) || !matches!(e[0], OutboundRouteEvent::ActivePilotChanged { .. })
        );
    }
}
