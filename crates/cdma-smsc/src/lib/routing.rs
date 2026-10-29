use std::fmt::{Display, Formatter};
use std::sync::Arc;

use cdma_hlr::repository::HlrRepository;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// ADDS Page requires an IMSI in the registration binding.
    Ready {
        imsi: String,
        serving_node: String,
        subscriber_id: Uuid,
    },
    Deferred {
        subscriber_id: Uuid,
    },
    Unknown,
    Unavailable(String),
}

pub struct Router {
    hlr: Arc<dyn HlrRepository>,
}

impl Router {
    pub fn new(hlr: Arc<dyn HlrRepository>) -> Self {
        Self { hlr }
    }

    pub async fn resolve_number(&self, phone_number: &str) -> Resolution {
        let resolved = match self.hlr.get_subscriber_by_phone_number(phone_number).await {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return Resolution::Unknown,
            Err(e) => {
                log::warn!("SMSC: HLR lookup failed for {phone_number}: {e}");
                return Resolution::Unavailable(e);
            }
        };
        let subscriber_id = resolved.subscriber.subscriber_id;
        let Some(binding) = resolved.binding.as_ref() else {
            return Resolution::Deferred { subscriber_id };
        };
        match binding.imsi.as_ref() {
            Some(imsi) => Resolution::Ready {
                imsi: imsi.clone(),
                serving_node: binding.serving_bs_id.clone(),
                subscriber_id,
            },
            None => Resolution::Deferred { subscriber_id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoSmsRoute {
    Local { subscriber_id: Uuid },
    Interconnect,
    Undeliverable(UndeliverableCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndeliverableCause {
    EmptyDestination,
    NoRoute,
    NotOurs,
}

impl UndeliverableCause {
    pub fn reason(self) -> &'static str {
        match self {
            Self::EmptyDestination => "no destination address",
            Self::NoRoute => "destination is not a subscriber and no interconnect is available",
            Self::NotOurs => {
                "arrived from another operator but the destination is not a subscriber here"
            }
        }
    }
}

impl Display for UndeliverableCause {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

pub fn route_mo_sms(
    destination: &str,
    local_subscriber: Option<Uuid>,
    interconnect_configured: bool,
) -> MoSmsRoute {
    route(
        destination,
        local_subscriber,
        interconnect_configured,
        false,
    )
}

/// Inbound interconnect messages must never be routed back out.
pub fn route(
    destination: &str,
    local_subscriber: Option<Uuid>,
    interconnect_configured: bool,
    from_interconnect: bool,
) -> MoSmsRoute {
    if destination.trim().is_empty() {
        return MoSmsRoute::Undeliverable(UndeliverableCause::EmptyDestination);
    }
    if let Some(subscriber_id) = local_subscriber {
        return MoSmsRoute::Local { subscriber_id };
    }
    if interconnect_configured && !from_interconnect {
        return MoSmsRoute::Interconnect;
    }
    MoSmsRoute::Undeliverable(if from_interconnect {
        UndeliverableCause::NotOurs
    } else {
        UndeliverableCause::NoRoute
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub() -> Uuid {
        Uuid::from_u128(1)
    }

    #[test]
    fn a_local_subscriber_wins() {
        assert_eq!(
            route_mo_sms("7025550100", Some(sub()), true),
            MoSmsRoute::Local {
                subscriber_id: sub()
            }
        );
    }

    #[test]
    fn an_unknown_number_goes_to_the_interconnect_when_one_is_bound() {
        assert_eq!(
            route_mo_sms("4155550118", None, true),
            MoSmsRoute::Interconnect
        );
    }

    #[test]
    fn an_unknown_number_fails_with_a_cause_when_no_interconnect_exists() {
        assert_eq!(
            route_mo_sms("4155550118", None, false),
            MoSmsRoute::Undeliverable(UndeliverableCause::NoRoute)
        );
    }

    #[test]
    fn an_empty_destination_fails_rather_than_being_offered_anywhere() {
        assert_eq!(
            route_mo_sms("", Some(sub()), true),
            MoSmsRoute::Undeliverable(UndeliverableCause::EmptyDestination)
        );
        assert_eq!(
            route_mo_sms("   ", None, true),
            MoSmsRoute::Undeliverable(UndeliverableCause::EmptyDestination)
        );
    }

    #[test]
    fn every_cause_states_a_reason() {
        for cause in [
            UndeliverableCause::EmptyDestination,
            UndeliverableCause::NoRoute,
        ] {
            assert!(!cause.reason().is_empty());
        }
    }
}
