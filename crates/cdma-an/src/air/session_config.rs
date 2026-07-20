//! HRPD session and protocol configuration negotiation: builds the AN's
//! ConfigurationResponse attributes and selects protocol subtypes from the
//! AT's ConfigurationRequest. Split out of the air module.

use super::*;

pub(super) fn configuration_response_attributes(
    protocol_type: u8,
    request_attributes: &[u8],
    reverse_traffic_mac_subtype: u16,
) -> Vec<u8> {
    match protocol_type {
        DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE => {
            default_session_configuration_response_attributes(request_attributes)
        }
        // C.S0024-0 v4.0: these default protocols define no configuration
        // attributes. An empty ConfigurationResponse leaves the default values
        // in effect when the AT requested no selectable record.
        SESSION_PROTOCOL_PHYSICAL_LAYER
        | SESSION_PROTOCOL_CONTROL_CHANNEL_MAC
        | SESSION_PROTOCOL_AUTHENTICATION
        | SESSION_PROTOCOL_ENCRYPTION
        | SESSION_PROTOCOL_SECURITY
        | SESSION_PROTOCOL_AIR_LINK_MANAGEMENT
        | SESSION_PROTOCOL_INITIALIZATION_STATE
        | DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE
        | DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE => Vec::new(),
        // C.S0024-400-C §2.6.7: DH Key Exchange has a one-octet KeyLength
        // attribute. We only advertise the 96-octet/768-bit mode until the
        // 1024-bit DH group is implemented end-to-end.
        SESSION_PROTOCOL_KEY_EXCHANGE => {
            fixed_width_configuration_response_attributes(request_attributes, 1, 1, Some(&[0x00]))
        }
        DEFAULT_IDLE_STATE_PROTOCOL_TYPE => {
            idle_state_configuration_response_attributes(request_attributes)
        }
        DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE => {
            route_update_configuration_response_attributes(request_attributes)
        }
        // Complex attributes: one-octet AttributeID followed by one-octet
        // ValueID. A response selects AttributeID+ValueID only.
        SESSION_PROTOCOL_ACCESS_CHANNEL_MAC => {
            fixed_width_configuration_response_attributes(request_attributes, 1, 1, None)
        }
        // Subtype-3 RTC MAC attribute records carry two-octet AttributeIDs
        // (C.S0024-A §10.11.7); default and subtype-1 records carry one-octet
        // AttributeIDs. Either way the response selects AttributeID + ValueID.
        SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC => {
            reverse_traffic_channel_mac_configuration_response_attributes(
                request_attributes,
                reverse_traffic_mac_subtype,
            )
        }
        SESSION_PROTOCOL_STREAM => stream_configuration_response_attributes(request_attributes),
        SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY => {
            multimode_capability_discovery_configuration_response_attributes(request_attributes)
        }
        // ForwardTrafficChannelMAC mixes simple DRCGating (0xff, 2-octet
        // value) with complex DRCLock (0x01, one-octet ValueID).
        SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC => {
            forward_traffic_channel_mac_configuration_response_attributes(request_attributes)
        }
        // Default Session Management has simple TSMPClose values in minutes.
        DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE => fixed_width_configuration_response_attributes(
            request_attributes,
            1,
            2,
            Some(&[0x0c, 0xa8]),
        ),
        // Default Packet attributes include RLP/flow-control values. Until we
        // implement them explicitly, keep the default/fallback values rather
        // than choosing the first advertised non-default value.
        SESSION_PROTOCOL_DEFAULT_PACKET_FIRST..=SESSION_PROTOCOL_DEFAULT_PACKET_LAST => Vec::new(),
        _ => {
            if !request_attributes.is_empty() {
                log::info!(
                    "HRPD AN: skipping {} configuration attributes for unsupported protocol; using default fallback",
                    stream0_protocol_name(protocol_type)
                );
            }
            Vec::new()
        }
    }
}

/// Decode and log a Subtype-3 RTC MAC Request message (C.S0024-A
/// §10.11.6.2.3: MaxSupportableTxT2P in 0.25 dB units plus per-MAC-flow
/// queue lengths). Returns false when the payload is not a Request.
pub(super) fn log_rtc_mac_request(uati: u32, protocol_type: u8, payload: &[u8]) -> bool {
    const RTC_MAC_REQUEST: u8 = 0x02;
    if protocol_type != SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC
        || payload.first() != Some(&RTC_MAC_REQUEST)
        || payload.len() < 3
    {
        return false;
    }
    let max_tx_t2p_quarter_db = payload[1];
    let num_flows = usize::from(payload[2] >> 3);
    let mut flows = Vec::with_capacity(num_flows);
    // MessageID(8) + MaxSupportableTxT2P(8) + NumMACFlows(5): the first
    // per-flow record starts at bit 21.
    let mut bit = 21usize;
    for _ in 0..num_flows {
        let take = |offset: usize, width: usize| -> u8 {
            let mut value = 0u8;
            for i in 0..width {
                let idx = offset + i;
                let byte = payload.get(idx / 8).copied().unwrap_or(0);
                value = (value << 1) | ((byte >> (7 - (idx % 8))) & 1);
            }
            value
        };
        let flow_id = take(bit, 4);
        let queue_len = take(bit + 4, 4);
        flows.push((flow_id, queue_len));
        bit += 8;
    }
    log::info!(
        "HRPD AN: RTC MAC Request UATI=0x{uati:08x} max_tx_t2p={:.2}dB flows={:?}",
        f64::from(max_tx_t2p_quarter_db) * 0.25,
        flows
    );
    true
}

/// True when the AT's SessionConfigurationRequest offers every protocol
/// subtype the Rev A personality requires. Subtype 2 Physical Layer defines
/// both link directions, so it is only selected together with Subtype 3 RTC
/// MAC and the Enhanced CC/AC/FTC MACs (C.S0024-A §10.3.1/§10.5.1/§10.7.1/
/// §10.11.1 pairing statements). Some ATs omit the top-level Route Update
/// selector while still configuring RouteUpdate attributes; those ATs keep the
/// default TCA grammar, with Rev A public data supplied by its optional tail.
pub(super) fn at_offers_rev_a_personality(request_attributes: &[u8]) -> bool {
    let offers = |protocol_type: u8, subtype: u16| {
        offered_session_protocol_values(request_attributes, protocol_type)
            .map(|values| {
                values
                    .chunks_exact(2)
                    .any(|value| subtype_to_u16(value) == subtype)
            })
            .unwrap_or(false)
    };
    offers(
        SESSION_PROTOCOL_PHYSICAL_LAYER,
        SESSION_SUBTYPE_PHYS_SUBTYPE2,
    ) && offers(
        SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC,
        SESSION_SUBTYPE_RTC_MAC_SUBTYPE3,
    ) && offers(
        SESSION_PROTOCOL_CONTROL_CHANNEL_MAC,
        SESSION_SUBTYPE_ENHANCED,
    ) && offers(
        SESSION_PROTOCOL_ACCESS_CHANNEL_MAC,
        SESSION_SUBTYPE_ENHANCED,
    ) && offers(
        SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC,
        SESSION_SUBTYPE_ENHANCED,
    )
}

/// Rev A personality subtype for one top-level Session Configuration
/// protocol attribute, or `None` when that protocol keeps its explicit/default
/// selection. Route Update is not added here because the live AT omits that
/// selector; the default TCA tail decision is made after commit.
pub(super) fn rev_a_session_protocol_subtype(protocol_type: u8) -> Option<u16> {
    match protocol_type {
        SESSION_PROTOCOL_PHYSICAL_LAYER => Some(SESSION_SUBTYPE_PHYS_SUBTYPE2),
        SESSION_PROTOCOL_CONTROL_CHANNEL_MAC
        | SESSION_PROTOCOL_ACCESS_CHANNEL_MAC
        | SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC => Some(SESSION_SUBTYPE_ENHANCED),
        _ => None,
    }
}

pub(super) fn default_session_configuration_response_attributes(
    request_attributes: &[u8],
) -> Vec<u8> {
    let mut response = Vec::new();
    let mut cursor = 0usize;
    let rev_a = at_offers_rev_a_personality(request_attributes);
    if rev_a {
        log::info!(
            "HRPD AN: AT offers the full Rev A personality; selecting physical subtype 2, enhanced CC/AC/FTC MAC, RTC MAC subtype 3"
        );
    }
    let selected_physical_layer_subtype = if rev_a {
        Some(SESSION_SUBTYPE_PHYS_SUBTYPE2)
    } else {
        match offered_session_protocol_values(request_attributes, SESSION_PROTOCOL_PHYSICAL_LAYER) {
            Some(values) => {
                select_supported_session_protocol_subtype(SESSION_PROTOCOL_PHYSICAL_LAYER, values)
                    .map(subtype_to_u16)
            }
            None => Some(SESSION_SUBTYPE_DEFAULT),
        }
    };
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 4 || end > request_attributes.len() {
            break;
        }
        let attr_start = cursor + 1;
        let attribute_id = &request_attributes[attr_start..attr_start + 2];
        let values = &request_attributes[attr_start + 2..end];
        if attribute_id[0] == 0 {
            let protocol_type = attribute_id[1];
            let rev_a_selection = if rev_a {
                rev_a_session_protocol_subtype(protocol_type)
            } else {
                None
            };
            let selected = if let Some(subtype) = rev_a_selection {
                log::info!(
                    "HRPD AN: selected SessionConfiguration protocol={} subtype=0x{subtype:04x} (Rev A personality)",
                    stream0_protocol_name(protocol_type)
                );
                Some(subtype)
            } else if protocol_type == SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC {
                selected_physical_layer_subtype.and_then(|physical_subtype| {
                    supported_reverse_traffic_mac_subtype(values, physical_subtype)
                })
            } else {
                supported_session_configuration_protocol_subtype(protocol_type, values)
            };
            if let Some(selected) = selected {
                response.push(4);
                response.extend_from_slice(attribute_id);
                response.extend_from_slice(&selected.to_be_bytes());
            } else {
                log::debug!(
                    "HRPD AN: skipping SessionConfiguration protocol={} attribute=0x{} offered={}; no implemented subtype offered",
                    stream0_protocol_name(protocol_type),
                    bytes_to_hex(attribute_id),
                    bytes_to_hex(values)
                );
            }
        } else if attribute_id == [0x10, 0x01] {
            if let Some(value_id) = supported_at_supported_application_subtypes_value_id(values) {
                // C.S0024-100-C §2.7.3.2: a ConfigurationResponse selecting
                // a complex attribute includes only the selected ValueID.
                response.push(3);
                response.extend_from_slice(attribute_id);
                response.push(value_id);
                log::info!(
                    "HRPD AN: selected SessionConfiguration ATSupportedApplicationSubtypes value_id=0x{value_id:02x} from values={}",
                    bytes_to_hex(values)
                );
            } else {
                log::debug!(
                    "HRPD AN: skipping SessionConfiguration ATSupportedApplicationSubtypes values={}; no value record contains an implemented packet application subtype",
                    bytes_to_hex(values)
                );
            }
        } else if let Some(selected) =
            supported_simple_session_attribute_value(attribute_id, values)
        {
            response.push(4);
            response.extend_from_slice(attribute_id);
            response.extend_from_slice(selected);
        } else {
            log::debug!(
                "HRPD AN: skipping SessionConfiguration attribute=0x{}; unsupported or relying on fallback",
                bytes_to_hex(attribute_id)
            );
        }
        cursor = end;
    }
    response
}

pub(super) fn offered_session_protocol_values(
    request_attributes: &[u8],
    protocol_type: u8,
) -> Option<&[u8]> {
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let length = usize::from(*request_attributes.get(cursor)?);
        let end = cursor.checked_add(1)?.checked_add(length)?;
        if length < 4 || end > request_attributes.len() {
            return None;
        }
        let attr_start = cursor + 1;
        if request_attributes[attr_start] == 0
            && request_attributes[attr_start + 1] == protocol_type
        {
            return Some(&request_attributes[attr_start + 2..end]);
        }
        cursor = end;
    }
    None
}

pub(super) fn select_supported_session_protocol_subtype(
    protocol_type: u8,
    values: &[u8],
) -> Option<&[u8]> {
    let supported = supported_session_protocol_subtypes(protocol_type)?;
    supported.iter().find_map(|supported_subtype| {
        values
            .chunks_exact(2)
            .find(|value| subtype_to_u16(value) == *supported_subtype)
    })
}

pub(super) fn supported_session_configuration_protocol_subtype(
    protocol_type: u8,
    values: &[u8],
) -> Option<u16> {
    let selected = select_supported_session_protocol_subtype(protocol_type, values);
    if let Some(selected) = selected {
        let selected = subtype_to_u16(selected);
        log::info!(
            "HRPD AN: selected SessionConfiguration protocol={} subtype=0x{:04x} from offered={}",
            stream0_protocol_name(protocol_type),
            selected,
            bytes_to_hex(values)
        );
        Some(selected)
    } else {
        log::debug!(
            "HRPD AN: unsupported SessionConfiguration protocol={} offered_subtypes={}",
            stream0_protocol_name(protocol_type),
            bytes_to_hex(values)
        );
        None
    }
}

pub(super) fn supported_reverse_traffic_mac_subtype(
    values: &[u8],
    physical_layer_subtype: u16,
) -> Option<u16> {
    let supported = reverse_traffic_mac_subtypes_for_physical_layer(physical_layer_subtype);
    let selected = supported.iter().find_map(|supported_subtype| {
        values
            .chunks_exact(2)
            .find(|value| subtype_to_u16(value) == *supported_subtype)
    });
    if let Some(selected) = selected {
        let selected = subtype_to_u16(selected);
        log::info!(
            "HRPD AN: selected SessionConfiguration protocol={} subtype=0x{:04x} from offered={} physical_subtype=0x{physical_layer_subtype:04x}",
            stream0_protocol_name(SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC),
            selected,
            bytes_to_hex(values)
        );
        Some(selected)
    } else {
        log::debug!(
            "HRPD AN: unsupported SessionConfiguration protocol={} offered_subtypes={} physical_subtype=0x{physical_layer_subtype:04x}",
            stream0_protocol_name(SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC),
            bytes_to_hex(values)
        );
        None
    }
}

pub(super) fn reverse_traffic_mac_subtypes_for_physical_layer(
    physical_layer_subtype: u16,
) -> &'static [u16] {
    match physical_layer_subtype {
        SESSION_SUBTYPE_DEFAULT | SESSION_SUBTYPE_REV0 => &[SESSION_SUBTYPE_DEFAULT],
        // Subtype 2 Physical Layer carries the sub-frame reverse link, which
        // only the Subtype 3 RTC MAC operates (C.S0024-A §10.11.1).
        SESSION_SUBTYPE_PHYS_SUBTYPE2 => &[SESSION_SUBTYPE_RTC_MAC_SUBTYPE3],
        _ => &[],
    }
}

pub(super) fn supported_session_protocol_subtypes(protocol_type: u8) -> Option<&'static [u16]> {
    match protocol_type {
        // C.S0024-0 §10.7 requires a ConfigurationResponse value to come from
        // the AT's offered list. If we cannot execute an offered non-default
        // subtype end-to-end, skip the attribute and let GCP fallback apply;
        // do not explicitly insert subtype 0 when the AT omitted it.
        SESSION_PROTOCOL_PHYSICAL_LAYER => Some(&[SESSION_SUBTYPE_DEFAULT, SESSION_SUBTYPE_REV0]),
        // The current AN/BTS common-channel paths execute the default
        // Control/Access Channel MAC procedures. Do not select subtype 1 until
        // enhanced common-channel configuration/probe procedures are
        // implemented end-to-end; falling back here is a deliberate protocol
        // boundary, not a traffic-channel shortcut.
        SESSION_PROTOCOL_CONTROL_CHANNEL_MAC | SESSION_PROTOCOL_ACCESS_CHANNEL_MAC => {
            Some(&[SESSION_SUBTYPE_DEFAULT])
        }
        SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC => Some(&[SESSION_SUBTYPE_DEFAULT]),
        SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC => Some(&[SESSION_SUBTYPE_DEFAULT]),
        // `security.rs` is still default pass-through. C.S0024-400-C §2.8
        // SHA-1 Authentication can declare Failed after commit if ACPAC does
        // not verify, and §2.4 Generic Security changes authenticated packet
        // framing. Per GCP §2.7, skip these non-default subtypes until their
        // committed-session wire behavior is implemented.
        SESSION_PROTOCOL_KEY_EXCHANGE
        | SESSION_PROTOCOL_AUTHENTICATION
        | SESSION_PROTOCOL_SECURITY => Some(&[SESSION_SUBTYPE_DEFAULT]),
        // GCP selection must choose from the AT's offered subtype list; some
        // ATs offer Enhanced Idle without the default subtype.
        DEFAULT_IDLE_STATE_PROTOCOL_TYPE => Some(&[SESSION_SUBTYPE_DEFAULT, SESSION_SUBTYPE_REV0]),
        DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE => {
            Some(&[SESSION_SUBTYPE_DEFAULT, SESSION_SUBTYPE_REV0])
        }
        DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE
        | DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
        | DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE
        | SESSION_PROTOCOL_STREAM => Some(&[SESSION_SUBTYPE_DEFAULT]),
        SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY => Some(&[SESSION_SUBTYPE_REV0]),
        _ => None,
    }
}

pub(super) fn subtype_to_u16(value: &[u8]) -> u16 {
    (u16::from(value[0]) << 8) | u16::from(value[1])
}

pub(super) fn session_config_selected_subtype(
    trace: Option<&SessionConfigTrace>,
    protocol_type: u8,
    protocol_subtype: u16,
) -> bool {
    let Some(trace) = trace else {
        return false;
    };
    let mut cursor = 0usize;
    while cursor < trace.response_attrs.len() {
        let Some(&length) = trace.response_attrs.get(cursor) else {
            return false;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            return false;
        };
        if length == 4 && end <= trace.response_attrs.len() {
            let attr = &trace.response_attrs[cursor + 1..cursor + 3];
            let value = &trace.response_attrs[cursor + 3..end];
            if attr == [0x00, protocol_type] && subtype_to_u16(value) == protocol_subtype {
                return true;
            }
        }
        cursor = end;
    }
    false
}

pub(super) fn session_config_selected_protocol_subtype(
    trace: Option<&SessionConfigTrace>,
    protocol_type: u8,
) -> Option<u16> {
    let trace = trace?;
    session_config_selected_u16_attribute(&trace.response_attrs, [0x00, protocol_type])
}

pub(super) fn session_config_selected_u16_attribute(
    response_attrs: &[u8],
    attribute_id: [u8; 2],
) -> Option<u16> {
    let mut cursor = 0usize;
    while cursor < response_attrs.len() {
        let length = usize::from(*response_attrs.get(cursor)?);
        let end = cursor.checked_add(1)?.checked_add(length)?;
        if length == 4 && end <= response_attrs.len() {
            let attr = &response_attrs[cursor + 1..cursor + 3];
            let value = &response_attrs[cursor + 3..end];
            if attr == attribute_id {
                return Some(subtype_to_u16(value));
            }
        }
        cursor = end;
    }
    None
}

pub(super) fn session_configuration_complete_payload(
    transaction_id: u8,
    personality_count: u16,
    commit_required: bool,
) -> (Vec<u8>, &'static str) {
    if personality_count > 1 {
        // C.S0024-500-C §5.4.6.2.5: PersonalityIndexStore=0,
        // Continue=0, Commit as selected by §5.4.6.1.7.2, token=0x0000.
        let control = if commit_required { 0x04 } else { 0x00 };
        let label = if commit_required {
            "SoftConfigurationComplete"
        } else {
            "SoftConfigurationCompleteNoCommit"
        };
        (
            vec![
                SESSION_SOFT_CONFIGURATION_COMPLETE,
                transaction_id,
                control,
                0x00,
                0x00,
            ],
            label,
        )
    } else {
        (
            vec![SESSION_CONFIGURATION_COMPLETE, transaction_id, 0x00, 0x00],
            "SessionConfigurationComplete",
        )
    }
}

pub(super) fn is_session_configuration_complete_label(label: &str) -> bool {
    matches!(
        label,
        "SessionConfigurationComplete"
            | "SoftConfigurationComplete"
            | "SoftConfigurationCompleteNoCommit"
    )
}

pub(super) fn session_configuration_complete_label_requires_close(label: &str) -> bool {
    matches!(
        label,
        "SessionConfigurationComplete" | "SoftConfigurationComplete"
    )
}

pub(super) fn cdma_system_time_80ms_now() -> u64 {
    time::system_time_20ms_frames(time::system_time_now()) / 4
}

pub(super) fn supported_simple_session_attribute_value<'a>(
    attribute_id: &[u8],
    values: &'a [u8],
) -> Option<&'a [u8]> {
    if attribute_id == SESSION_ATTRIBUTE_PERSONALITY_COUNT {
        // Only select a value we intend to maintain as live session state. If
        // the AT offers only multi-personality values, skip this attribute and
        // let the GCP fallback PersonalityCount=1 apply.
        return values
            .chunks_exact(2)
            .find(|value| subtype_to_u16(value) == SESSION_PERSONALITY_COUNT_DEFAULT);
    }
    let preferred: &[[u8; 2]] = match attribute_id {
        // SessionConfigurationToken and SupportGAUPSessionConfigurationToken.
        [0x01, 0x00] | [0x01, 0x01] => &[[0x00, 0x00]],
        // SupportConfigurationLock.
        [0x01, 0x02] => &[[0x00, 0x00]],
        _ => return None,
    };
    preferred
        .iter()
        .find_map(|preferred| values.chunks_exact(2).find(|value| *value == *preferred))
}

pub(super) fn supported_at_supported_application_subtypes_value_id(values: &[u8]) -> Option<u8> {
    let mut cursor = 0usize;
    while cursor + 2 <= values.len() {
        let value_id = values[cursor];
        let count = usize::from(values[cursor + 1]);
        let subtypes_start = cursor + 2;
        let Some(record_len) = count.checked_mul(2).and_then(|len| len.checked_add(2)) else {
            return None;
        };
        let record_end = cursor.checked_add(record_len)?;
        if record_end > values.len() {
            return None;
        }
        let subtypes = &values[subtypes_start..record_end];
        if subtypes
            .chunks_exact(2)
            .any(|subtype| subtype_to_u16(subtype) == DEFAULT_PACKET_SERVICE_NETWORK_SUBTYPE)
        {
            return Some(value_id);
        }
        cursor = record_end;
    }
    None
}

pub(super) fn fixed_width_configuration_response_attributes(
    request_attributes: &[u8],
    attribute_id_octets: usize,
    selected_value_octets: usize,
    preferred_value: Option<&[u8]>,
) -> Vec<u8> {
    let mut response = Vec::new();
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < attribute_id_octets + selected_value_octets || end > request_attributes.len() {
            break;
        }
        let attr_start = cursor + 1;
        let value_start = attr_start + attribute_id_octets;
        let attribute_id = &request_attributes[attr_start..value_start];
        let values = &request_attributes[value_start..end];
        let selected = preferred_value
            .and_then(|preferred| {
                values
                    .chunks_exact(selected_value_octets)
                    .find(|value| *value == preferred)
            })
            .or_else(|| values.get(..selected_value_octets));
        if let Some(selected) = selected {
            let response_len = attribute_id_octets + selected_value_octets;
            let Ok(response_len) = u8::try_from(response_len) else {
                break;
            };
            response.push(response_len);
            response.extend_from_slice(attribute_id);
            response.extend_from_slice(selected);
        }
        cursor = end;
    }
    response
}

pub(super) fn reverse_traffic_channel_mac_configuration_response_attributes(
    request_attributes: &[u8],
    reverse_traffic_mac_subtype: u16,
) -> Vec<u8> {
    if reverse_traffic_mac_subtype != SESSION_SUBTYPE_RTC_MAC_SUBTYPE3 {
        return fixed_width_configuration_response_attributes(request_attributes, 1, 1, None);
    }

    // Skip every subtype-3 record, including MaxMACFlows (0x0014): the GCP
    // fallback keeps both sides at the spec defaults, and accepting the AT's
    // wider MaxMACFlows would only grow the set of attributes the AN must
    // honor over GAUP while it serves flows 0/1 only.
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 3 || end > request_attributes.len() {
            log::debug!(
                "HRPD AN: skipping malformed subtype3 RTC MAC configuration record at offset={cursor} len={length}; using default fallback"
            );
            break;
        }

        let attr = subtype_to_u16(&request_attributes[cursor + 1..cursor + 3]);
        let values = &request_attributes[cursor + 3..end];
        log::info!(
            "HRPD AN: skipping subtype3 RTC MAC configuration attribute=0x{attr:04x} offered={}; using default fallback",
            bytes_to_hex(values)
        );
        cursor = end;
    }

    Vec::new()
}

pub(super) fn idle_state_configuration_response_attributes(request_attributes: &[u8]) -> Vec<u8> {
    // PreferredControlChannelCycle is the Default Idle State complex attribute
    // used by live ATs during setup. The ConfigurationResponse selects the
    // AttributeID plus the AT-offered ValueID, not the full value record.
    one_octet_complex_value_id_response(request_attributes, &[0x00])
}

pub(super) fn selected_idle_preferred_control_channel_cycle(
    response_attributes: &[u8],
) -> Option<u16> {
    let mut cursor = 0usize;
    while cursor < response_attributes.len() {
        let length = usize::from(*response_attributes.get(cursor)?);
        let end = cursor.checked_add(1)?.checked_add(length)?;
        if length < 3 || end > response_attributes.len() {
            return None;
        }
        let attribute_id = response_attributes[cursor + 1];
        if attribute_id == IDLE_STATE_ATTRIBUTE_PREFERRED_CONTROL_CHANNEL_CYCLE {
            let value = &response_attributes[cursor + 2..end];
            let raw = (u16::from(value[0]) << 8) | u16::from(value[1]);
            if raw & 0x8000 != 0 {
                return Some(raw & 0x7fff);
            }
            return None;
        }
        cursor = end;
    }
    None
}

pub(super) fn route_update_configuration_response_attributes(request_attributes: &[u8]) -> Vec<u8> {
    // SupportedCDMAChannels is the Default Route Update complex attribute the
    // AT is allowed to send in a ConfigurationRequest.
    one_octet_complex_value_id_response(request_attributes, &[0x04])
}

pub(super) fn multimode_capability_discovery_configuration_response_attributes(
    request_attributes: &[u8],
) -> Vec<u8> {
    let mut response = Vec::new();
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 2 || end > request_attributes.len() {
            break;
        }
        let attribute_id = request_attributes[cursor + 1];
        let values = &request_attributes[cursor + 2..end];
        if let Some(value) = values
            .iter()
            .copied()
            .find(|value| mcd_attribute_value_is_valid(attribute_id, *value))
        {
            // C.S0024-500-C §5.5.5.2.2 plus C.S0024-100-C §2.7:
            // MCD attributes are simple one-octet values, so the response
            // selects one offered value. This is not an AN-proposed new value.
            response.extend_from_slice(&[0x02, attribute_id, value]);
            log::info!(
                "HRPD AN: selected MCD {} value=0x{value:02x} from offered={}",
                mcd_attribute_name(attribute_id),
                bytes_to_hex(values)
            );
        } else {
            log::warn!(
                "HRPD AN: skipping MCD {} attribute=0x{attribute_id:02x} offered={}; no valid offered value",
                mcd_attribute_name(attribute_id),
                bytes_to_hex(values)
            );
        }
        cursor = end;
    }
    response
}

pub(super) fn mcd_attribute_value_is_valid(attribute_id: u8, value: u8) -> bool {
    match attribute_id {
        MCD_SIMULTANEOUS_COMMON_CHANNEL_TRANSMIT | MCD_SIMULTANEOUS_DEDICATED_CHANNEL_TRANSMIT => {
            value <= 0x0b
        }
        MCD_SIMULTANEOUS_COMMON_CHANNEL_RECEIVE | MCD_SIMULTANEOUS_DEDICATED_CHANNEL_RECEIVE => {
            value <= 0x09
        }
        MCD_HYBRID_MS_AT | MCD_RECEIVER_DIVERSITY => value <= 0x01,
        _ => false,
    }
}

pub(super) fn mcd_attribute_name(attribute_id: u8) -> &'static str {
    match attribute_id {
        MCD_SIMULTANEOUS_COMMON_CHANNEL_TRANSMIT => "SimultaneousCommonChannelTransmit",
        MCD_SIMULTANEOUS_DEDICATED_CHANNEL_TRANSMIT => "SimultaneousDedicatedChannelTransmit",
        MCD_SIMULTANEOUS_COMMON_CHANNEL_RECEIVE => "SimultaneousCommonChannelReceive",
        MCD_SIMULTANEOUS_DEDICATED_CHANNEL_RECEIVE => "SimultaneousDedicatedChannelReceive",
        MCD_HYBRID_MS_AT => "HybridMSAT",
        MCD_RECEIVER_DIVERSITY => "ReceiverDiversity",
        _ => "unknown",
    }
}

pub(super) fn one_octet_complex_value_id_response(
    request_attributes: &[u8],
    allowed_attrs: &[u8],
) -> Vec<u8> {
    let mut response = Vec::new();
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 2 || end > request_attributes.len() {
            break;
        }
        let attribute_id = request_attributes[cursor + 1];
        let value_id = request_attributes[cursor + 2];
        if allowed_attrs.contains(&attribute_id) {
            response.extend_from_slice(&[0x02, attribute_id, value_id]);
        }
        cursor = end;
    }
    response
}

pub(super) fn canonical_protocol_configuration_response(
    protocol_type: u8,
    request_attributes: &[u8],
    response_attributes: &[u8],
) -> Vec<u8> {
    if protocol_type == SESSION_PROTOCOL_STREAM {
        canonical_stream_configuration_selection(request_attributes, response_attributes)
    } else {
        canonical_configuration_selection(request_attributes, response_attributes)
    }
}

pub(super) fn canonical_configuration_selection(
    request_attributes: &[u8],
    response_attributes: &[u8],
) -> Vec<u8> {
    let mut canonical = Vec::new();
    let mut cursor = 0usize;
    while cursor < response_attributes.len() {
        let Some(&length) = response_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            canonical.extend_from_slice(&response_attributes[cursor..]);
            break;
        };
        if length == 0 || end > response_attributes.len() {
            canonical.extend_from_slice(&response_attributes[cursor..]);
            break;
        }
        if length == 2 {
            let attribute_id = response_attributes[cursor + 1];
            let value_id = response_attributes[cursor + 2];
            if let Some(selected_value) =
                one_octet_complex_selected_value(request_attributes, attribute_id, value_id)
            {
                let canonical_len = selected_value.len() + 1;
                if let Ok(canonical_len) = u8::try_from(canonical_len) {
                    canonical.push(canonical_len);
                    canonical.push(attribute_id);
                    canonical.extend_from_slice(selected_value);
                    cursor = end;
                    continue;
                }
            }
        }
        canonical.extend_from_slice(&response_attributes[cursor..end]);
        cursor = end;
    }
    canonical
}

pub(super) fn one_octet_complex_selected_value(
    request_attributes: &[u8],
    attribute_id: u8,
    value_id: u8,
) -> Option<&[u8]> {
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let length = usize::from(*request_attributes.get(cursor)?);
        let end = cursor.checked_add(1)?.checked_add(length)?;
        if length > 2
            && end <= request_attributes.len()
            && request_attributes[cursor + 1] == attribute_id
            && request_attributes[cursor + 2] == value_id
        {
            return Some(&request_attributes[cursor + 3..end]);
        }
        cursor = end;
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectedDefaultPacketStream {
    pub(super) value_id: u8,
    pub(super) stream_id: u8,
    pub(super) protocol_type: u8,
    pub(super) application_subtype: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamConfigurationResponse {
    pub(super) attributes: Vec<u8>,
    pub(super) default_packet: Option<SelectedDefaultPacketStream>,
}

pub(super) fn stream_configuration_response_attributes(request_attributes: &[u8]) -> Vec<u8> {
    stream_configuration_response(request_attributes).attributes
}

pub(super) fn stream_configuration_response(
    request_attributes: &[u8],
) -> StreamConfigurationResponse {
    let mut response = Vec::new();
    let mut selected_default_packet = None;
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 10 || end > request_attributes.len() {
            break;
        }
        let attr_start = cursor + 1;
        let attribute_id = request_attributes[attr_start];
        let records = &request_attributes[attr_start + 1..end];
        if attribute_id == 0x00 {
            for record in records.chunks_exact(9) {
                if let Some(selection) = default_packet_stream_selection(record) {
                    log::info!(
                        "HRPD AN: selected StreamConfiguration value_id=0x{:02x} Stream{}Application=0x{:04x} (DefaultPacket service-network)",
                        selection.value_id,
                        selection.stream_id,
                        selection.application_subtype
                    );
                    response.extend_from_slice(&[0x02, attribute_id, record[0]]);
                    selected_default_packet = Some(selection);
                    break;
                }
            }
        }
        cursor = end;
    }
    if response.is_empty() && !request_attributes.is_empty() {
        log::warn!(
            "HRPD AN: Stream ConfigurationRequest had no supported DefaultPacket service-network mapping; using fallback"
        );
    }
    StreamConfigurationResponse {
        attributes: response,
        default_packet: selected_default_packet,
    }
}

pub(super) fn canonical_stream_configuration_selection(
    request_attributes: &[u8],
    response_attributes: &[u8],
) -> Vec<u8> {
    let mut canonical = Vec::new();
    let mut cursor = 0usize;
    while cursor < response_attributes.len() {
        let Some(&length) = response_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            canonical.extend_from_slice(&response_attributes[cursor..]);
            break;
        };
        if length == 0 || end > response_attributes.len() {
            canonical.extend_from_slice(&response_attributes[cursor..]);
            break;
        }
        if length == 2 && response_attributes[cursor + 1] == 0x00 {
            let value_id = response_attributes[cursor + 2];
            if let Some(record) = stream_configuration_record(request_attributes, value_id) {
                let selected = &record[1..];
                let canonical_len = selected.len() + 1;
                if let Ok(canonical_len) = u8::try_from(canonical_len) {
                    canonical.push(canonical_len);
                    canonical.push(0x00);
                    canonical.extend_from_slice(selected);
                    cursor = end;
                    continue;
                }
            }
        }
        canonical.extend_from_slice(&response_attributes[cursor..end]);
        cursor = end;
    }
    canonical
}

pub(super) fn stream_configuration_record(
    request_attributes: &[u8],
    value_id: u8,
) -> Option<&[u8]> {
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let length = usize::from(*request_attributes.get(cursor)?);
        let end = cursor.checked_add(1)?.checked_add(length)?;
        if length < 10 || end > request_attributes.len() {
            return None;
        }
        let attr_start = cursor + 1;
        let attribute_id = request_attributes[attr_start];
        let records = &request_attributes[attr_start + 1..end];
        if attribute_id == 0x00 {
            for record in records.chunks_exact(9) {
                if record.first().copied() == Some(value_id) {
                    return Some(record);
                }
            }
        }
        cursor = end;
    }
    None
}

pub(super) fn default_packet_stream_selection(
    record: &[u8],
) -> Option<SelectedDefaultPacketStream> {
    let value_id = *record.first()?;
    for (stream_id, offset) in [(1u8, 3usize), (2, 5), (3, 7)] {
        let application_subtype = subtype_to_u16(record.get(offset..offset + 2)?);
        if application_subtype != DEFAULT_PACKET_SERVICE_NETWORK_SUBTYPE {
            continue;
        }
        let protocol_type = default_packet_stream_protocol_type(stream_id)?;
        return Some(SelectedDefaultPacketStream {
            value_id,
            stream_id,
            protocol_type,
            application_subtype,
        });
    }
    None
}

pub(super) fn forward_traffic_channel_mac_configuration_response_attributes(
    request_attributes: &[u8],
) -> Vec<u8> {
    let mut response = Vec::new();
    let mut cursor = 0usize;
    while cursor < request_attributes.len() {
        let Some(&length) = request_attributes.get(cursor) else {
            break;
        };
        let length = usize::from(length);
        let Some(end) = cursor
            .checked_add(1)
            .and_then(|start| start.checked_add(length))
        else {
            break;
        };
        if length < 2 || end > request_attributes.len() {
            break;
        }
        let attr_start = cursor + 1;
        let attribute_id = request_attributes[attr_start];
        let values = &request_attributes[attr_start + 1..end];
        match attribute_id {
            0xff => {
                if let Some(selected) = values
                    .chunks_exact(2)
                    .find(|value| *value == [0x00, 0x00])
                    .or_else(|| values.chunks_exact(2).next())
                {
                    response.push(3);
                    response.push(attribute_id);
                    response.extend_from_slice(selected);
                }
            }
            0x01 => {
                if let Some(&value_id) = values.first() {
                    response.push(2);
                    response.push(attribute_id);
                    response.push(value_id);
                }
            }
            _ => {
                log::info!(
                    "HRPD AN: skipping ForwardTrafficChannelMAC unknown attribute=0x{attribute_id:02x}"
                );
            }
        }
        cursor = end;
    }
    response
}
