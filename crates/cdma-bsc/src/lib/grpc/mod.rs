mod service;
mod state;

/// Starts the BSC management gRPC server on `addr`.
pub use service::run_grpc_server;
/// Shared BSC runtime state exposed to the management gRPC service.
pub use state::BscState;

/// Generated types for the BSC service proto (`bsc.v1`).
pub mod proto {
    tonic::include_proto!("bsc.v1");
}

/// Generated types for the shared management proto (`management.v1`).
pub mod management_proto {
    tonic::include_proto!("management.v1");
}

/// Generated types for the BTS management proto (`bts_management.v1`).
pub mod bts_management_proto {
    tonic::include_proto!("bts_management.v1");
}

/// Generated types for the BSC management proto (`bsc_management.v1`).
pub mod bsc_management_proto {
    tonic::include_proto!("bsc_management.v1");
}

/// Generated types for the MSC management proto (`msc_management.v1`).
pub mod msc_management_proto {
    tonic::include_proto!("msc_management.v1");
}

/// Generated types for the PCF management proto (`pcf_management.v1`).
pub mod pcf_management_proto {
    tonic::include_proto!("pcf_management.v1");
}

/// Generated types for the PDSN management proto (`pdsn_management.v1`).
pub mod pdsn_management_proto {
    tonic::include_proto!("pdsn_management.v1");
}

/// Generated types for the HLR service proto (`hlr.v1`).
pub mod hlr_proto {
    tonic::include_proto!("hlr.v1");
}

/// Generated types for the SMSC service proto (`smsc.v1`).
pub mod smsc_proto {
    tonic::include_proto!("smsc.v1");
}

/// Generated types for the packet service proto (`packet.v1`).
pub mod packet_proto {
    tonic::include_proto!("packet.v1");
}
