//! Traffic-channel event DTOs and telemetry formatting.

use cdma_common::formatting::{bitstream_to_hex, forward_order_name};
use cdma_common::lac::{
    MessageControlStatusBlock,
    message_types::MessageId,
    paging_messages::{
        AlertWithInformationMessage, ForwardDataBurstMessage, MsAddress, OrderMessage,
        ServiceConnectParams, ServiceRequestParams,
    },
};
use cdma_common::sms as air_sms;

use crate::addressing::format_ms_address;

use super::{Bsc, next_bsc_event_id};

/// Forward-link traffic channel signaling event for gRPC/UI streaming.
#[derive(Debug, Clone)]
pub struct TrafficEvent {
    pub event_id: String,
    pub walsh_code: u8,
    pub service_option: Option<u16>,
    pub rc_label: Option<String>,
    pub mcsb: MessageControlStatusBlock,
    pub timestamp_us: u64,
    pub address: String,
    pub l3_summary: Option<String>,
    pub pdu_summary: String,
    pub sdu_hex: String,
    pub pdu_hex: String,
    pub order: Option<OrderMessage>,
    pub service_connect: Option<ServiceConnectParams>,
    pub service_request: Option<ServiceRequestParams>,
    pub data_burst: Option<ForwardDataBurstMessage>,
    pub alert_with_info: Option<AlertWithInformationMessage>,
    /// Voice call sub-state at the time this event was emitted.
    pub voice_call_state: Option<String>,
}

pub(crate) fn traffic_event_l3_summary(
    msg_id: MessageId,
    order: &Option<OrderMessage>,
    data_burst: &Option<ForwardDataBurstMessage>,
    alert_with_info: &Option<AlertWithInformationMessage>,
) -> Option<String> {
    if let Some(message) = order.as_ref() {
        Some(if message.ordq == 0 {
            forward_order_name(message.order).to_string()
        } else {
            format!(
                "{} | ORDQ={}",
                forward_order_name(message.order),
                message.ordq
            )
        })
    } else if msg_id == MessageId::ServiceConnect {
        Some("Service Connect Message".to_string())
    } else if msg_id == MessageId::AlertWithInformation {
        Some(if let Some(awim) = alert_with_info.as_ref() {
            if let Some(si) = awim.signal_info.as_ref() {
                if si.signal == 0x3F {
                    "Alert With Information (Tones Off)".to_string()
                } else {
                    "Alert With Information (Ringback)".to_string()
                }
            } else {
                "Alert With Information".to_string()
            }
        } else {
            "Alert With Information".to_string()
        })
    } else if msg_id == MessageId::DataBurst {
        if let Some(db) = data_burst.as_ref() {
            if db.burst_type == 3 {
                air_sms::decode_mt_sms(&db.fields).map(|d| {
                    if d.tl_msg_type == 0x02 {
                        format!(
                            "SMS Cause Code (reply_seq={}, error={})",
                            d.reply_seq.unwrap_or(0),
                            d.error_class.unwrap_or(0)
                        )
                    } else {
                        let truncated = if d.text.len() > 40 {
                            format!("{}…", &d.text[..40])
                        } else {
                            d.text.clone()
                        };
                        format!(
                            "SMS Deliver | from={} | \"{}\"",
                            d.originating_number, truncated
                        )
                    }
                })
            } else {
                Some(format!("Data Burst (type={})", db.burst_type))
            }
        } else {
            Some("Data Burst".to_string())
        }
    } else {
        None
    }
}

impl Bsc {
    pub(crate) fn emit_traffic_tx_event(
        &self,
        walsh_code: u8,
        service_option: u16,
        rc_label: &str,
        frame_count: usize,
        mcsb: MessageControlStatusBlock,
        addr: &MsAddress,
        sdu: &cdma_common::bits::Bitstream,
        pdu: &cdma_common::bits::Bitstream,
        order: Option<OrderMessage>,
        service_connect: Option<ServiceConnectParams>,
        service_request: Option<ServiceRequestParams>,
        data_burst: Option<ForwardDataBurstMessage>,
        alert_with_info: Option<AlertWithInformationMessage>,
        voice_call_state: Option<String>,
    ) {
        let message_id = mcsb.message_id;
        self.events.publish_traffic_event(TrafficEvent {
            event_id: next_bsc_event_id("traffic"),
            walsh_code,
            service_option: Some(service_option),
            rc_label: Some(rc_label.to_string()),
            mcsb,
            timestamp_us: chrono::Utc::now().timestamp_micros() as u64,
            address: format_ms_address(addr),
            l3_summary: traffic_event_l3_summary(message_id, &order, &data_burst, &alert_with_info),
            pdu_summary: format!(
                "walsh={} rc={} service_option={} pdu_bits={} frames={}",
                walsh_code,
                rc_label,
                service_option,
                pdu.len(),
                frame_count,
            ),
            sdu_hex: bitstream_to_hex(sdu),
            pdu_hex: bitstream_to_hex(pdu),
            order,
            service_connect,
            service_request,
            data_burst,
            alert_with_info,
            voice_call_state,
        });
    }
}
