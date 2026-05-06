use cdma_abis::Error;
use cdma_abis::bearer::{ChannelFamily, Direction};
use cdma_abis::udp_bearer::{
    BearerRouteKey, UdpBearerDatagram, UdpBearerRouteOutcome, UdpBearerRouter,
};

fn datagram(sequence_no: u32, bearer_id: u32) -> UdpBearerDatagram {
    UdpBearerDatagram {
        flags: 0,
        channel_family: ChannelFamily::Fch,
        direction: Direction::Forward,
        bts_id: 10,
        cell_id: 20,
        bearer_id,
        sequence_no,
        tx_frame_number: 30,
        payload: vec![0xaa, 0xbb],
    }
}

fn route_key(bearer_id: u32) -> BearerRouteKey {
    BearerRouteKey {
        channel_family: ChannelFamily::Fch,
        direction: Direction::Forward,
        bts_id: 10,
        cell_id: 20,
        bearer_id,
    }
}

#[test]
fn udp_bearer_decode_rejects_non_zero_reserved_bytes() {
    let mut encoded = datagram(1, 100).encode().unwrap();
    encoded[24] = 1;
    let error = UdpBearerDatagram::decode(&encoded).unwrap_err();
    assert_eq!(
        error,
        Error::InvalidValue {
            context: "Abis UDP bearer header",
            reason: "reserved header octets must be zero",
        }
    );
}

#[test]
fn udp_bearer_router_accepts_registered_route() {
    let mut router = UdpBearerRouter::default();
    router.register_route(route_key(100));
    let routed = router.route(datagram(1, 100)).unwrap();
    assert_eq!(routed.key, route_key(100));
    assert_eq!(routed.outcome, UdpBearerRouteOutcome::Accepted);
}

#[test]
fn udp_bearer_router_rejects_unknown_route() {
    let mut router = UdpBearerRouter::default();
    let error = router.route(datagram(1, 100)).unwrap_err();
    assert_eq!(
        error,
        Error::InvalidValue {
            context: "Abis UDP bearer route",
            reason: "datagram does not match a registered bearer",
        }
    );
}

#[test]
fn udp_bearer_router_drops_duplicates_and_late_packets() {
    let mut router = UdpBearerRouter::default();
    router.register_route(route_key(100));

    assert_eq!(
        router.route(datagram(10, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
    assert_eq!(
        router.route(datagram(10, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::DuplicateDrop
    );
    assert_eq!(
        router.route(datagram(9, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::LateDrop
    );
    assert_eq!(
        router.counters(route_key(100)).unwrap(),
        cdma_abis::udp_bearer::UdpBearerRouteCounters {
            accepted: 1,
            duplicate_drop: 1,
            late_drop: 1,
        }
    );
}

#[test]
fn udp_bearer_router_accepts_sequence_wrap() {
    let mut router = UdpBearerRouter::default();
    router.register_route(route_key(100));

    assert_eq!(
        router.route(datagram(u32::MAX, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
    assert_eq!(
        router.route(datagram(0, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
    assert_eq!(
        router.route(datagram(u32::MAX, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::DuplicateDrop
    );
}

#[test]
fn udp_bearer_router_tracks_routes_independently() {
    let mut router = UdpBearerRouter::default();
    router.register_route(route_key(100));
    router.register_route(route_key(200));

    assert_eq!(
        router.route(datagram(7, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
    assert_eq!(
        router.route(datagram(7, 200)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
    assert_eq!(
        router.route(datagram(6, 200)).unwrap().outcome,
        UdpBearerRouteOutcome::LateDrop
    );
    assert_eq!(
        router.route(datagram(8, 100)).unwrap().outcome,
        UdpBearerRouteOutcome::Accepted
    );
}
