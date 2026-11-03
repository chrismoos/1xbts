use super::*;
use crate::sip::tests::{config, mo_invite_request, otasp_message, otasp_test_core};
use cdma_ios::{AddsDeliverMessage, AddsUserPart, CellId, Tag};
use cdma_sip::SipMessage;
use tokio::sync::mpsc;

const DELIVERY_TAG: u32 = 0x8000_0001;
const INVALID_CSEQ: &str = "999 ACK";

fn burst(call_id: u64) -> EncodedA1Message {
    let deliver = AddsDeliverMessage {
        adds_user_part: AddsUserPart {
            burst_type: BURST_TYPE_OTASP,
            data: vec![6],
        },
        tag: Some(Tag(DELIVERY_TAG)),
    };
    EncodedA1Message::from_message_for_call(
        &Message::new(MessageType::AddsDeliver, deliver.encode().unwrap()),
        Some(call_id),
    )
}

fn delivery_ack(method: Method) -> SipRequest {
    let ack = cdma_ios::AddsDeliverAckMessage {
        tag: Some(Tag(DELIVERY_TAG)),
        cause: None,
    };
    let mut request = otasp_message(
        &cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_adds_deliver_ack(&ack).unwrap(),
    );
    request.headers.set("CSeq", format!("11 {method}"));
    request.method = method;
    request
}

struct AnsweredCall {
    core: ImsCore,
    rx: mpsc::Receiver<EncodedA1Message>,
    invite: SipRequest,
    answer: SipResponse,
    peer: SocketAddr,
    call_id: u64,
}

impl AnsweredCall {
    fn new() -> Self {
        let (tx, mut rx) = mpsc::channel(4);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 403,
                sector: 0,
            },
            Some(tx),
        );
        let invite = mo_invite_request();
        let peer = "198.51.100.1:5060".parse().unwrap();
        assert_eq!(
            core.handle_mo_invite(&invite, peer).status,
            super::super::SC_TRYING
        );
        let call_id = rx.try_recv().unwrap().call_id().unwrap();
        let (answer, _) = core.stage_otasp_answer(&burst(call_id)).unwrap().unwrap();
        Self {
            core,
            rx,
            invite,
            answer,
            peer,
            call_id,
        }
    }

    fn ack(&self) -> SipRequest {
        let mut ack = self.invite.clone();
        ack.method = Method::Ack;
        ack.headers.set("CSeq", "1 ACK");
        ack.headers.set("To", self.answer.headers.to().unwrap());
        ack
    }
}

#[tokio::test]
async fn lost_answer_or_ack_retransmits_until_matching_ack() {
    let call = AnsweredCall::new();
    let local = "127.0.0.1:5060".parse().unwrap();
    let first_retry = call.core.pending_otasp_answer.lock().unwrap()[&call.call_id].next_retry;
    assert!(
        call.core
            .otasp_answer_timer(first_retry - SIP_T1, local)
            .is_empty()
    );
    let retry = call.core.otasp_answer_timer(first_retry, local);
    assert_eq!(retry.len(), 1);
    let SipMessage::Response(answer) = &retry[0].0 else {
        panic!("expected retransmitted answer")
    };
    assert_eq!(answer, &call.answer);
    assert_eq!(retry[0].1, call.peer);
    assert_eq!(
        call.core.handle_mo_invite(&call.invite, call.peer),
        call.answer
    );
    let mut now = first_retry;
    for expected_interval in [SIP_T1 * 2, SIP_T1 * 4, SIP_T2, SIP_T2] {
        let next = call.core.pending_otasp_answer.lock().unwrap()[&call.call_id].next_retry;
        assert_eq!(next - now, expected_interval);
        assert_eq!(call.core.otasp_answer_timer(next, local).len(), 1);
        now = next;
    }
    let bursts = call.core.otasp_after_ack(&call.ack(), call.peer).unwrap();
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0].call_id(), Some(call.call_id));
    assert!(call.core.otasp_after_ack(&call.ack(), call.peer).is_none());
    assert!(call.core.otasp_answer_timer(now + SIP_T2, local).is_empty());
}

#[tokio::test]
async fn wrong_cseq_tags_or_peer_do_not_consume_pending_bursts() {
    let call = AnsweredCall::new();
    let valid = call.ack();
    let mut invalid = valid.clone();
    invalid.headers.set("CSeq", INVALID_CSEQ);
    assert!(call.core.otasp_after_ack(&invalid, call.peer).is_none());
    invalid = valid.clone();
    invalid.headers.remove("CSeq");
    assert!(call.core.otasp_after_ack(&invalid, call.peer).is_none());
    for header in ["From", "To"] {
        invalid = valid.clone();
        invalid
            .headers
            .set(header, "<sip:other@example.test>;tag=wrong");
        assert!(call.core.otasp_after_ack(&invalid, call.peer).is_none());
        invalid.headers.set(header, "<sip:other@example.test>");
        assert!(call.core.otasp_after_ack(&invalid, call.peer).is_none());
    }
    assert!(
        call.core
            .otasp_after_ack(&valid, "198.51.100.2:5060".parse().unwrap())
            .is_none()
    );
    call.core
        .stage_otasp_answer(&burst(call.call_id))
        .unwrap()
        .unwrap();
    assert_eq!(
        call.core.otasp_after_ack(&valid, call.peer).unwrap().len(),
        2
    );
}

#[tokio::test]
async fn unacknowledged_answer_times_out_and_clears_sip_and_a1_call() {
    let mut call = AnsweredCall::new();
    let deadline = call.core.pending_otasp_answer.lock().unwrap()[&call.call_id].expires;
    let outgoing = call
        .core
        .otasp_answer_timer(deadline, "127.0.0.1:5060".parse().unwrap());
    assert_eq!(outgoing.len(), 1);
    let SipMessage::Request(bye) = &outgoing[0].0 else {
        panic!("expected BYE")
    };
    assert_eq!(bye.method, Method::Bye);
    let clear = call.rx.try_recv().unwrap();
    assert_eq!(clear.message_type(), MessageType::ClearRequest);
    assert_eq!(clear.call_id(), Some(call.call_id));
    assert!(call.core.pending_otasp_answer.lock().unwrap().is_empty());
    assert!(
        !call
            .core
            .mo_voice_by_call
            .lock()
            .unwrap()
            .contains_key(&call.call_id)
    );
    assert!(
        call.core
            .otasp_answer_timer(deadline + SIP_T2, "127.0.0.1:5060".parse().unwrap())
            .is_empty()
    );
    assert!(call.rx.try_recv().is_err());
}

#[tokio::test]
async fn duplicate_info_and_message_ack_replay_success_without_forwarding_twice() {
    for method in [Method::Info, Method::Message] {
        let (core, mut rx) = otasp_test_core();
        core.adds_delivery_calls
            .lock()
            .unwrap()
            .insert(("fap-a".into(), DELIVERY_TAG), 41);
        let request = delivery_ack(method);
        let peer = "198.51.100.1:5060".parse().unwrap();
        let first = core.handle_request_from(&request, peer);
        assert_eq!(first.status, SC_OK);
        assert_eq!(
            rx.try_recv().unwrap().message_type(),
            MessageType::AddsDeliverAck
        );
        assert_eq!(core.handle_request_from(&request, peer), first);
        assert!(rx.try_recv().is_err());
        assert_ne!(
            core.handle_request_from(&request, "198.51.100.2:5060".parse().unwrap())
                .status,
            SC_OK
        );
        let mut other = request.clone();
        other
            .headers
            .set("Via", "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKother");
        assert_eq!(
            core.handle_request_from(&other, peer).status,
            SC_BAD_REQUEST
        );
        core.adds_ack_responses
            .lock()
            .unwrap()
            .expire(Instant::now() + SIP_INVITE_TIMEOUT);
        assert_eq!(
            core.handle_request_from(&request, peer).status,
            SC_BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn full_msc_queue_preserves_delivery_tag_for_retry() {
    let (core, mut rx) = otasp_test_core();
    let sender = core.msc_a1_tx.as_ref().unwrap();
    while sender.try_send(burst(41)).is_ok() {}
    core.adds_delivery_calls
        .lock()
        .unwrap()
        .insert(("fap-a".into(), DELIVERY_TAG), 41);
    let request = delivery_ack(Method::Info);
    let peer = "198.51.100.1:5060".parse().unwrap();
    assert_eq!(
        core.handle_request_from(&request, peer).status,
        super::super::SC_SERVICE_UNAVAILABLE
    );
    assert!(
        core.adds_delivery_calls
            .lock()
            .unwrap()
            .contains_key(&("fap-a".into(), DELIVERY_TAG))
    );
    while rx.try_recv().is_ok() {}
    assert_eq!(core.handle_request_from(&request, peer).status, SC_OK);
    assert_eq!(
        rx.try_recv().unwrap().message_type(),
        MessageType::AddsDeliverAck
    );
    assert!(rx.try_recv().is_err());
}
