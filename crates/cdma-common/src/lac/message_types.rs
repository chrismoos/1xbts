/// Unified message identity for all CDMA2000 signaling channels.
///
/// Each variant represents a unique message type from the IS-2000 / C.S0004-E
/// spec. The enum abstracts away channel-specific wire encodings: use
/// [`MessageId::wire_type`] and [`MessageId::from_wire`] to convert between
/// the enum and the per-logical-channel numeric MSG_TYPE / MSG_TAG values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageId {
    // ---- Sync channel ----
    SyncChannelMessage, // SCHM

    // ---- Forward common signaling (f-csch / paging) ----
    SystemParameters, // SPM
    AccessParameters, // APM
    NeighborList,     // NLM
    CdmaChannelList,  // CCLM
    // Historic/legacy paging messages that are not assigned in C.S0004-E f-csch Table 3.1.2.3.1.1.2-1.
    SlottedPage,
    Page,
    ChannelAssignment,
    AuthChallenge,
    SsdUpdate,
    FeatureNotification,
    ExtSystemParameters, // ESPM
    ExtNeighborList,     // ENLM
    StatusRequest,
    ServiceRedirection,
    GeneralPage, // GPM
    GlobalServiceRedirection,
    TmsiAssignment,
    Paca,
    ExtChannelAssignment, // ECAM
    GeneralNeighborList,  // GNLM
    UserZoneIdentification,
    PrivateNeighborList,
    UserZoneReject,
    ExtGlobalServiceRedirection,
    ExtCdmaChannelList,                 // ECCLM
    Ansi41SystemParameters,             // A41SPM
    McRrParameters,                     // MCRRPM
    Ansi41Rand,                         // A41RANDM
    EnhancedAccessParameters,           // EAPM
    UniversalNeighborList,              // UNLM
    SecurityModeCommand,                // SMCM
    UniversalPage,                      // UPM
    UniversalPageFirstSegment,          // UPM first segment
    UniversalPageMiddleSegment,         // UPM middle segment
    UniversalPageFinalSegment,          // UPM final segment
    McMapSyncChannel,                   // MAPSCHM
    McMapSystemInformation,             // MAPSIM
    McMapFlowRelease,                   // MAPFRM
    RTmsiAssignment,                    // RTASM
    AuthenticationRequest,              // AUREQM
    BroadcastServiceParameters,         // BSPM
    MeidExtChannelAssignment,           // MECAM
    AlternativeTechnologiesInformation, // ATIM
    AccessPointIdentifier,              // APIDM
    AccessPointIdentifierText,          // APIDTM
    AccessPointPilotInformation,        // APPIM
    GeneralOverheadInformation,         // GOIM
    FlexDuplexCdmaChannelList,          // FDCCLM

    // ---- Shared forward/reverse ----
    Order,     // ORDM
    DataBurst, // DBM

    // ---- Reverse common signaling (r-csch / access) ----
    Registration,
    Origination,
    PageResponse,
    PacaCancel,
    ExtStatusResponse,
    Reconnect,
    RadioEnvironment,
    CallRecoveryRequest,

    // ---- Shared reverse common + reverse dedicated ----
    AuthChallengeResponse,    // AUCRM
    StatusResponse,           // STRPM
    TmsiAssignmentCompletion, // TACM
    DeviceInformation,        // DIM
    SecurityModeRequest,      // SMRM
    AuthResponse,             // AURSPM
    AuthResync,               // AURSYNM
    GeneralExtension,         // GEM

    // ---- Forward dedicated (f-dsch) only ----
    AlertWithInformation,                  // AWIM
    ServiceConnect,                        // SCM
    ExtendedSupplementalChannelAssignment, // ESCAM

    // ---- Reverse dedicated (r-dsch) only ----
    FlashWithInfo,               // FWIM
    Psmm,                        // PSMM
    PowerMeasurementReport,      // PMRM
    SendBurstDtmf,               // BDTMFM
    Status,                      // STM
    OriginationContinuation,     // ORCM
    HandoffCompletion,           // HOCM
    ParametersResponse,          // PRSM
    ServiceRequest,              // SRQM
    ServiceResponse,             // SRPM
    ServiceConnectCompletion,    // SCCM
    ServiceOptionControl,        // SOCM
    SupplementalChannelRequest,  // SCRM
    CandidateFreqSearchResponse, // CFSRSM
    CandidateFreqSearchReport,   // CFSRPM
    PeriodicPsmm,                // PPSMM
    OuterLoopReport,             // OLRM
    ResourceRequest,             // RRM
    ExtReleaseResponse,          // ERRM
    EnhancedOrigination,         // EOM
    ExtFlashWithInfo,            // EFWIM
    ExtPsmm,                     // EPSMM
    ExtHandoffCompletion,        // EHOCM
    ResourceReleaseRequest,      // RRRM
    DataBurstResponse,           // DBRM
    Ds41IntersystemTransfer,     // D41ISTM
    UserZoneUpdateRequest,       // UZURM
    CallCancel,                  // CLCM
    McmapInitialL3,              // MAPIL3M
    McmapL3,                     // MAPL3M
    RTmsiAssignmentCompletion,   // RTACM
    BsStatusRequest,             // BSSREQM
    CdmaOfftimeReport,           // COTRM
    ItbspmRequest,               // ITBSPMRM
    HandoffSuppInfoNotification, // HOSINM
}

/// Signaling channel category for wire encoding/decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireChannel {
    /// Sync Channel (SCH): sync logical channel.
    Sync,
    /// Forward common signaling channel (f-csch): paging/broadcast.
    ForwardCommon,
    /// Reverse common signaling channel (r-csch): access channel.
    ReverseCommon,
    /// Forward dedicated signaling channel (f-dsch): traffic forward.
    ForwardDedicated,
    /// Reverse dedicated signaling channel (r-dsch): traffic reverse.
    ReverseDedicated,
}

impl MessageId {
    /// Human-readable message name.
    pub fn name(self) -> &'static str {
        match self {
            Self::SyncChannelMessage => "Sync Channel Message",
            Self::SystemParameters => "System Parameters Message",
            Self::AccessParameters => "Access Parameters Message",
            Self::NeighborList => "Neighbor List Message",
            Self::CdmaChannelList => "CDMA Channel List Message",
            Self::SlottedPage => "Slotted Page Message",
            Self::Page => "Page Message",
            Self::Order => "Order Message",
            Self::ChannelAssignment => "Channel Assignment Message",
            Self::DataBurst => "Data Burst Message",
            Self::AuthChallenge => "Authentication Challenge Message",
            Self::SsdUpdate => "SSD Update Message",
            Self::FeatureNotification => "Feature Notification Message",
            Self::ExtSystemParameters => "Extended System Parameters Message",
            Self::ExtNeighborList => "Extended Neighbor List Message",
            Self::StatusRequest => "Status Request Message",
            Self::ServiceRedirection => "Service Redirection Message",
            Self::GeneralPage => "General Page Message",
            Self::GlobalServiceRedirection => "Global Service Redirection Message",
            Self::TmsiAssignment => "TMSI Assignment Message",
            Self::Paca => "PACA Message",
            Self::ExtChannelAssignment => "Extended Channel Assignment Message",
            Self::GeneralNeighborList => "General Neighbor List Message",
            Self::UserZoneIdentification => "User Zone Identification Message",
            Self::PrivateNeighborList => "Private Neighbor List Message",
            Self::UserZoneReject => "User Zone Reject Message",
            Self::ExtGlobalServiceRedirection => "Extended Global Service Redirection Message",
            Self::ExtCdmaChannelList => "Extended CDMA Channel List Message",
            Self::Ansi41SystemParameters => "ANSI-41 System Parameters Message",
            Self::McRrParameters => "MC-RR Parameters Message",
            Self::Ansi41Rand => "ANSI-41 RAND Message",
            Self::EnhancedAccessParameters => "Enhanced Access Parameters Message",
            Self::UniversalNeighborList => "Universal Neighbor List Message",
            Self::SecurityModeCommand => "Security Mode Command Message",
            Self::UniversalPage => "Universal Page Message",
            Self::UniversalPageFirstSegment => "Universal Page Message First Segment",
            Self::UniversalPageMiddleSegment => "Universal Page Message Middle Segment",
            Self::UniversalPageFinalSegment => "Universal Page Message Final Segment",
            Self::McMapSyncChannel => "MC-MAP Sync Channel Message",
            Self::McMapSystemInformation => "MC-MAP System Information Message",
            Self::McMapFlowRelease => "MC-MAP Flow Release Message",
            Self::RTmsiAssignment => "R-TMSI Assignment Message",
            Self::AuthenticationRequest => "Authentication Request Message",
            Self::BroadcastServiceParameters => "Broadcast Service Parameters Message",
            Self::MeidExtChannelAssignment => "MEID Extended Channel Assignment Message",
            Self::AlternativeTechnologiesInformation => {
                "Alternative Technologies Information Message"
            }
            Self::AccessPointIdentifier => "Access Point Identifier Message",
            Self::AccessPointIdentifierText => "Access Point Identifier Text Message",
            Self::AccessPointPilotInformation => "Access Point Pilot Information Message",
            Self::GeneralOverheadInformation => "General Overhead Information Message",
            Self::FlexDuplexCdmaChannelList => "Flex Duplex CDMA Channel List Message",
            Self::Registration => "Registration Message",
            Self::Origination => "Origination Message",
            Self::PageResponse => "Page Response Message",
            Self::AuthChallengeResponse => "Auth Challenge Response Message",
            Self::StatusResponse => "Status Response Message",
            Self::TmsiAssignmentCompletion => "TMSI Assignment Completion Message",
            Self::PacaCancel => "PACA Cancel Message",
            Self::ExtStatusResponse => "Extended Status Response Message",
            Self::DeviceInformation => "Device Information Message",
            Self::SecurityModeRequest => "Security Mode Request Message",
            Self::AuthResponse => "Auth Response Message",
            Self::AuthResync => "Auth Resynchronization Message",
            Self::Reconnect => "Reconnect Message",
            Self::RadioEnvironment => "Radio Environment Message",
            Self::CallRecoveryRequest => "Call Recovery Request Message",
            Self::GeneralExtension => "General Extension Message",
            Self::AlertWithInformation => "Alert With Information Message",
            Self::ServiceConnect => "Service Connect Message",
            Self::ExtendedSupplementalChannelAssignment => {
                "Extended Supplemental Channel Assignment Message"
            }
            Self::FlashWithInfo => "Flash with Information Message",
            Self::Psmm => "Pilot Strength Measurement Message",
            Self::PowerMeasurementReport => "Power Measurement Report Message",
            Self::SendBurstDtmf => "Send Burst DTMF Message",
            Self::Status => "Status Message",
            Self::OriginationContinuation => "Origination Continuation Message",
            Self::HandoffCompletion => "Handoff Completion Message",
            Self::ParametersResponse => "Parameters Response Message",
            Self::ServiceRequest => "Service Request Message",
            Self::ServiceResponse => "Service Response Message",
            Self::ServiceConnectCompletion => "Service Connect Completion Message",
            Self::ServiceOptionControl => "Service Option Control Message",
            Self::SupplementalChannelRequest => "Supplemental Channel Request Message",
            Self::CandidateFreqSearchResponse => "Candidate Freq Search Response Message",
            Self::CandidateFreqSearchReport => "Candidate Freq Search Report Message",
            Self::PeriodicPsmm => "Periodic PSMM",
            Self::OuterLoopReport => "Outer Loop Report Message",
            Self::ResourceRequest => "Resource Request Message",
            Self::ExtReleaseResponse => "Extended Release Response Message",
            Self::EnhancedOrigination => "Enhanced Origination Message",
            Self::ExtFlashWithInfo => "Extended Flash with Information Message",
            Self::ExtPsmm => "Extended PSMM",
            Self::ExtHandoffCompletion => "Extended Handoff Completion Message",
            Self::ResourceReleaseRequest => "Resource Release Request Message",
            Self::DataBurstResponse => "Data Burst Response Message",
            Self::Ds41IntersystemTransfer => "DS-41 Inter-system Transfer Message",
            Self::UserZoneUpdateRequest => "User Zone Update Request Message",
            Self::CallCancel => "Call Cancel Message",
            Self::McmapInitialL3 => "MC-MAP Initial L3 Message",
            Self::McmapL3 => "MC-MAP L3 Message",
            Self::RTmsiAssignmentCompletion => "R-TMSI Assignment Completion Message",
            Self::BsStatusRequest => "Base Station Status Request Message",
            Self::CdmaOfftimeReport => "CDMA Offtime Report Message",
            Self::ItbspmRequest => "ITBSPM Request Message",
            Self::HandoffSuppInfoNotification => "Handoff Supp Info Notification Message",
        }
    }

    /// Short symbolic tag (3-6 chars) matching spec abbreviations.
    pub fn tag(self) -> &'static str {
        match self {
            Self::SyncChannelMessage => "SCHM",
            Self::SystemParameters => "SPM",
            Self::AccessParameters => "APM",
            Self::NeighborList => "NLM",
            Self::CdmaChannelList => "CCLM",
            Self::SlottedPage => "SPM_SL",
            Self::Page => "PM",
            Self::Order => "ORDM",
            Self::ChannelAssignment => "CAM",
            Self::DataBurst => "DBM",
            Self::AuthChallenge => "AUCM",
            Self::SsdUpdate => "SSDUM",
            Self::FeatureNotification => "FNM",
            Self::ExtSystemParameters => "ESPM",
            Self::ExtNeighborList => "ENLM",
            Self::StatusRequest => "STRQM",
            Self::ServiceRedirection => "SRDM",
            Self::GeneralPage => "GPM",
            Self::GlobalServiceRedirection => "GSRDM",
            Self::TmsiAssignment => "TASM",
            Self::Paca => "PACAM",
            Self::ExtChannelAssignment => "ECAM",
            Self::GeneralNeighborList => "GNLM",
            Self::UserZoneIdentification => "UZIM",
            Self::PrivateNeighborList => "PNLM",
            Self::UserZoneReject => "UZRM",
            Self::ExtGlobalServiceRedirection => "EGSRDM",
            Self::ExtCdmaChannelList => "ECCLM",
            Self::Ansi41SystemParameters => "A41SPM",
            Self::McRrParameters => "MCRRPM",
            Self::Ansi41Rand => "A41RANDM",
            Self::EnhancedAccessParameters => "EAPM",
            Self::UniversalNeighborList => "UNLM",
            Self::SecurityModeCommand => "SMCM",
            Self::UniversalPage => "UPM",
            Self::UniversalPageFirstSegment => "UPM",
            Self::UniversalPageMiddleSegment => "UPM",
            Self::UniversalPageFinalSegment => "UPM",
            Self::McMapSyncChannel => "MAPSCHM",
            Self::McMapSystemInformation => "MAPSIM",
            Self::McMapFlowRelease => "MAPFRM",
            Self::RTmsiAssignment => "RTASM",
            Self::AuthenticationRequest => "AUREQM",
            Self::BroadcastServiceParameters => "BSPM",
            Self::MeidExtChannelAssignment => "MECAM",
            Self::AlternativeTechnologiesInformation => "ATIM",
            Self::AccessPointIdentifier => "APIDM",
            Self::AccessPointIdentifierText => "APIDTM",
            Self::AccessPointPilotInformation => "APPIM",
            Self::GeneralOverheadInformation => "GOIM",
            Self::FlexDuplexCdmaChannelList => "FDCCLM",
            Self::Registration => "REGM",
            Self::Origination => "ORIGM",
            Self::PageResponse => "PRM",
            Self::AuthChallengeResponse => "AUCRM",
            Self::StatusResponse => "STRPM",
            Self::TmsiAssignmentCompletion => "TACM",
            Self::PacaCancel => "PACNM",
            Self::ExtStatusResponse => "ESTRPM",
            Self::DeviceInformation => "DIM",
            Self::SecurityModeRequest => "SMRM",
            Self::AuthResponse => "AURSPM",
            Self::AuthResync => "AURSYNM",
            Self::Reconnect => "RCNM",
            Self::RadioEnvironment => "REM",
            Self::CallRecoveryRequest => "CRRM",
            Self::GeneralExtension => "GEM",
            Self::AlertWithInformation => "AWIM",
            Self::ServiceConnect => "SCM",
            Self::ExtendedSupplementalChannelAssignment => "ESCAM",
            Self::FlashWithInfo => "FWIM",
            Self::Psmm => "PSMM",
            Self::PowerMeasurementReport => "PMRM",
            Self::SendBurstDtmf => "BDTMFM",
            Self::Status => "STM",
            Self::OriginationContinuation => "ORCM",
            Self::HandoffCompletion => "HOCM",
            Self::ParametersResponse => "PRSM",
            Self::ServiceRequest => "SRQM",
            Self::ServiceResponse => "SRPM",
            Self::ServiceConnectCompletion => "SCCM",
            Self::ServiceOptionControl => "SOCM",
            Self::SupplementalChannelRequest => "SCRM",
            Self::CandidateFreqSearchResponse => "CFSRSM",
            Self::CandidateFreqSearchReport => "CFSRPM",
            Self::PeriodicPsmm => "PPSMM",
            Self::OuterLoopReport => "OLRM",
            Self::ResourceRequest => "RRM",
            Self::ExtReleaseResponse => "ERRM",
            Self::EnhancedOrigination => "EOM",
            Self::ExtFlashWithInfo => "EFWIM",
            Self::ExtPsmm => "EPSMM",
            Self::ExtHandoffCompletion => "EHOCM",
            Self::ResourceReleaseRequest => "RRRM",
            Self::DataBurstResponse => "DBRM",
            Self::Ds41IntersystemTransfer => "D41ISTM",
            Self::UserZoneUpdateRequest => "UZURM",
            Self::CallCancel => "CLCM",
            Self::McmapInitialL3 => "MAPIL3M",
            Self::McmapL3 => "MAPL3M",
            Self::RTmsiAssignmentCompletion => "RTACM",
            Self::BsStatusRequest => "BSSREQM",
            Self::CdmaOfftimeReport => "COTRM",
            Self::ItbspmRequest => "ITBSPMRM",
            Self::HandoffSuppInfoNotification => "HOSINM",
        }
    }

    /// Channel-specific wire value for this message, or `None` if the message
    /// does not exist on the given channel.
    pub fn wire_type(self, ch: WireChannel) -> Option<u8> {
        match ch {
            WireChannel::Sync => self.wire_sync(),
            WireChannel::ForwardCommon => self.wire_forward_common(),
            WireChannel::ReverseCommon => self.wire_reverse_common(),
            WireChannel::ForwardDedicated => self.wire_forward_dedicated(),
            WireChannel::ReverseDedicated => self.wire_reverse_dedicated(),
        }
    }

    /// Decode a wire value from the given channel into a `MessageId`.
    pub fn from_wire(ch: WireChannel, raw: u8) -> Option<Self> {
        match ch {
            WireChannel::Sync => Self::from_wire_sync(raw),
            WireChannel::ForwardCommon => Self::from_wire_forward_common(raw),
            WireChannel::ReverseCommon => Self::from_wire_reverse_common(raw),
            WireChannel::ForwardDedicated => Self::from_wire_forward_dedicated(raw),
            WireChannel::ReverseDedicated => Self::from_wire_reverse_dedicated(raw),
        }
    }

    // -- Sync Channel (SCH): C.S0004-E Table 3.1.2.3.1.1.2-1, logical channel sync --

    fn wire_sync(self) -> Option<u8> {
        Some(match self {
            Self::SyncChannelMessage => 0x01,
            _ => return None,
        })
    }

    fn from_wire_sync(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::SyncChannelMessage,
            _ => return None,
        })
    }

    // -- Forward common (f-csch / paging): C.S0004-E Table 3.1.2.3.1.1.2-1 --

    fn wire_forward_common(self) -> Option<u8> {
        Some(match self {
            Self::SystemParameters => 0x01,
            Self::AccessParameters => 0x02,
            Self::NeighborList => 0x03,
            Self::CdmaChannelList => 0x04,
            Self::Order => 0x07,
            Self::ChannelAssignment => 0x08,
            Self::DataBurst => 0x09,
            Self::AuthChallenge => 0x0A,
            Self::SsdUpdate => 0x0B,
            Self::FeatureNotification => 0x0C,
            Self::ExtSystemParameters => 0x0D,
            Self::ExtNeighborList => 0x0E,
            Self::StatusRequest => 0x0F,
            Self::ServiceRedirection => 0x10,
            Self::GeneralPage => 0x11,
            Self::GlobalServiceRedirection => 0x12,
            Self::TmsiAssignment => 0x13,
            Self::Paca => 0x14,
            Self::ExtChannelAssignment => 0x15,
            Self::GeneralNeighborList => 0x16,
            Self::UserZoneIdentification => 0x17,
            Self::PrivateNeighborList => 0x18,
            Self::UserZoneReject => 0x1C,
            Self::Ansi41SystemParameters => 0x1D,
            Self::McRrParameters => 0x1E,
            Self::Ansi41Rand => 0x1F,
            Self::EnhancedAccessParameters => 0x20,
            Self::UniversalNeighborList => 0x21,
            Self::SecurityModeCommand => 0x22,
            Self::UniversalPage => 0x23,
            Self::UniversalPageFirstSegment => 0x24,
            Self::UniversalPageMiddleSegment => 0x25,
            Self::UniversalPageFinalSegment => 0x26,
            Self::McMapSyncChannel => 0x27,
            Self::McMapSystemInformation => 0x28,
            Self::McmapL3 => 0x29,
            Self::RTmsiAssignment => 0x2A,
            Self::McMapFlowRelease => 0x2B,
            Self::AuthenticationRequest => 0x2C,
            Self::BroadcastServiceParameters => 0x2D,
            Self::MeidExtChannelAssignment => 0x2E,
            Self::AlternativeTechnologiesInformation => 0x2F,
            Self::AccessPointIdentifier => 0x30,
            Self::AccessPointIdentifierText => 0x31,
            Self::AccessPointPilotInformation => 0x32,
            Self::GeneralOverheadInformation => 0x33,
            Self::FlexDuplexCdmaChannelList => 0x34,
            Self::GeneralExtension => 0x3F,
            Self::ExtGlobalServiceRedirection => 0x1A,
            Self::ExtCdmaChannelList => 0x1B,
            _ => return None,
        })
    }

    fn from_wire_forward_common(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::SystemParameters,
            0x02 => Self::AccessParameters,
            0x03 => Self::NeighborList,
            0x04 => Self::CdmaChannelList,
            0x07 => Self::Order,
            0x08 => Self::ChannelAssignment,
            0x09 => Self::DataBurst,
            0x0A => Self::AuthChallenge,
            0x0B => Self::SsdUpdate,
            0x0C => Self::FeatureNotification,
            0x0D => Self::ExtSystemParameters,
            0x0E => Self::ExtNeighborList,
            0x0F => Self::StatusRequest,
            0x10 => Self::ServiceRedirection,
            0x11 => Self::GeneralPage,
            0x12 => Self::GlobalServiceRedirection,
            0x13 => Self::TmsiAssignment,
            0x14 => Self::Paca,
            0x15 => Self::ExtChannelAssignment,
            0x16 => Self::GeneralNeighborList,
            0x17 => Self::UserZoneIdentification,
            0x18 => Self::PrivateNeighborList,
            0x1A => Self::ExtGlobalServiceRedirection,
            0x1B => Self::ExtCdmaChannelList,
            0x1C => Self::UserZoneReject,
            0x1D => Self::Ansi41SystemParameters,
            0x1E => Self::McRrParameters,
            0x1F => Self::Ansi41Rand,
            0x20 => Self::EnhancedAccessParameters,
            0x21 => Self::UniversalNeighborList,
            0x22 => Self::SecurityModeCommand,
            0x23 => Self::UniversalPage,
            0x24 => Self::UniversalPageFirstSegment,
            0x25 => Self::UniversalPageMiddleSegment,
            0x26 => Self::UniversalPageFinalSegment,
            0x27 => Self::McMapSyncChannel,
            0x28 => Self::McMapSystemInformation,
            0x29 => Self::McmapL3,
            0x2A => Self::RTmsiAssignment,
            0x2B => Self::McMapFlowRelease,
            0x2C => Self::AuthenticationRequest,
            0x2D => Self::BroadcastServiceParameters,
            0x2E => Self::MeidExtChannelAssignment,
            0x2F => Self::AlternativeTechnologiesInformation,
            0x30 => Self::AccessPointIdentifier,
            0x31 => Self::AccessPointIdentifierText,
            0x32 => Self::AccessPointPilotInformation,
            0x33 => Self::GeneralOverheadInformation,
            0x34 => Self::FlexDuplexCdmaChannelList,
            0x3F => Self::GeneralExtension,
            _ => return None,
        })
    }

    // -- Reverse common (r-csch / access): C.S0004-E Table 2.1.1.4.1.1.2-1 --

    fn wire_reverse_common(self) -> Option<u8> {
        Some(match self {
            Self::Registration => 0x01,
            Self::Order => 0x02,
            Self::DataBurst => 0x03,
            Self::Origination => 0x04,
            Self::PageResponse => 0x05,
            Self::AuthChallengeResponse => 0x06,
            Self::StatusResponse => 0x07,
            Self::TmsiAssignmentCompletion => 0x08,
            Self::PacaCancel => 0x09,
            Self::ExtStatusResponse => 0x0A,
            Self::DeviceInformation => 0x0D,
            Self::SecurityModeRequest => 0x0E,
            Self::AuthResponse => 0x15,
            Self::AuthResync => 0x16,
            Self::Reconnect => 0x17,
            Self::RadioEnvironment => 0x18,
            Self::CallRecoveryRequest => 0x19,
            Self::GeneralExtension => 0x3F,
            _ => return None,
        })
    }

    fn from_wire_reverse_common(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::Registration,
            0x02 => Self::Order,
            0x03 => Self::DataBurst,
            0x04 => Self::Origination,
            0x05 => Self::PageResponse,
            0x06 => Self::AuthChallengeResponse,
            0x07 => Self::StatusResponse,
            0x08 => Self::TmsiAssignmentCompletion,
            0x09 => Self::PacaCancel,
            0x0A => Self::ExtStatusResponse,
            0x0D => Self::DeviceInformation,
            0x0E => Self::SecurityModeRequest,
            0x15 => Self::AuthResponse,
            0x16 => Self::AuthResync,
            0x17 => Self::Reconnect,
            0x18 => Self::RadioEnvironment,
            0x19 => Self::CallRecoveryRequest,
            0x3F => Self::GeneralExtension,
            _ => return None,
        })
    }

    // -- Forward dedicated (f-dsch): C.S0004-E Table 3.2.2.2.1.2-1 --

    fn wire_forward_dedicated(self) -> Option<u8> {
        Some(match self {
            Self::Order => 0x01,                                 // 00000001 ORDRM
            Self::AuthChallenge => 0x02,                         // 00000010 AUCM
            Self::AlertWithInformation => 0x03,                  // 00000011 AWIM
            Self::DataBurst => 0x04,                             // 00000100 DBM
            Self::FlashWithInfo => 0x0E,                         // 00001110 FWIM
            Self::ServiceRequest => 0x12,                        // 00010010 SRQM
            Self::ServiceResponse => 0x13,                       // 00010011 SRPM
            Self::ServiceConnect => 0x14,                        // 00010100 SCM
            Self::ExtendedSupplementalChannelAssignment => 0x22, // 00100010 ESCAM
            _ => return None,
        })
    }

    fn from_wire_forward_dedicated(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::Order,
            0x02 => Self::AuthChallenge,
            0x03 => Self::AlertWithInformation,
            0x04 => Self::DataBurst,
            0x0E => Self::FlashWithInfo,
            0x12 => Self::ServiceRequest,
            0x13 => Self::ServiceResponse,
            0x14 => Self::ServiceConnect,
            0x22 => Self::ExtendedSupplementalChannelAssignment,
            _ => return None,
        })
    }

    // -- Reverse dedicated (r-dsch): C.S0004-E Table 2.2.1.2.1.2-1 --

    fn wire_reverse_dedicated(self) -> Option<u8> {
        Some(match self {
            Self::Order => 0x01,
            Self::AuthChallengeResponse => 0x02,
            Self::FlashWithInfo => 0x03,
            Self::DataBurst => 0x04,
            Self::Psmm => 0x05,
            Self::PowerMeasurementReport => 0x06,
            Self::SendBurstDtmf => 0x07,
            Self::Status => 0x08,
            Self::OriginationContinuation => 0x09,
            Self::HandoffCompletion => 0x0A,
            Self::ParametersResponse => 0x0B,
            Self::ServiceRequest => 0x0C,
            Self::ServiceResponse => 0x0D,
            Self::ServiceConnectCompletion => 0x0E,
            Self::ServiceOptionControl => 0x0F,
            Self::StatusResponse => 0x10,
            Self::TmsiAssignmentCompletion => 0x11,
            Self::SupplementalChannelRequest => 0x12,
            Self::CandidateFreqSearchResponse => 0x13,
            Self::CandidateFreqSearchReport => 0x14,
            Self::PeriodicPsmm => 0x15,
            Self::OuterLoopReport => 0x16,
            Self::ResourceRequest => 0x17,
            Self::ExtReleaseResponse => 0x18,
            Self::EnhancedOrigination => 0x1A,
            Self::ExtFlashWithInfo => 0x1B,
            Self::ExtPsmm => 0x1C,
            Self::ExtHandoffCompletion => 0x1D,
            Self::ResourceReleaseRequest => 0x1E,
            Self::SecurityModeRequest => 0x1F,
            Self::DataBurstResponse => 0x20,
            Self::Ds41IntersystemTransfer => 0x21,
            Self::UserZoneUpdateRequest => 0x22,
            Self::CallCancel => 0x23,
            Self::DeviceInformation => 0x24,
            Self::McmapInitialL3 => 0x25,
            Self::McmapL3 => 0x26,
            Self::RTmsiAssignmentCompletion => 0x27,
            Self::BsStatusRequest => 0x28,
            Self::CdmaOfftimeReport => 0x29,
            Self::AuthResync => 0x2A,
            Self::AuthResponse => 0x2B,
            Self::ItbspmRequest => 0x2C,
            Self::HandoffSuppInfoNotification => 0x2D,
            Self::GeneralExtension => 0xFF,
            _ => return None,
        })
    }

    fn from_wire_reverse_dedicated(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::Order,
            0x02 => Self::AuthChallengeResponse,
            0x03 => Self::FlashWithInfo,
            0x04 => Self::DataBurst,
            0x05 => Self::Psmm,
            0x06 => Self::PowerMeasurementReport,
            0x07 => Self::SendBurstDtmf,
            0x08 => Self::Status,
            0x09 => Self::OriginationContinuation,
            0x0A => Self::HandoffCompletion,
            0x0B => Self::ParametersResponse,
            0x0C => Self::ServiceRequest,
            0x0D => Self::ServiceResponse,
            0x0E => Self::ServiceConnectCompletion,
            0x0F => Self::ServiceOptionControl,
            0x10 => Self::StatusResponse,
            0x11 => Self::TmsiAssignmentCompletion,
            0x12 => Self::SupplementalChannelRequest,
            0x13 => Self::CandidateFreqSearchResponse,
            0x14 => Self::CandidateFreqSearchReport,
            0x15 => Self::PeriodicPsmm,
            0x16 => Self::OuterLoopReport,
            0x17 => Self::ResourceRequest,
            0x18 => Self::ExtReleaseResponse,
            0x1A => Self::EnhancedOrigination,
            0x1B => Self::ExtFlashWithInfo,
            0x1C => Self::ExtPsmm,
            0x1D => Self::ExtHandoffCompletion,
            0x1E => Self::ResourceReleaseRequest,
            0x1F => Self::SecurityModeRequest,
            0x20 => Self::DataBurstResponse,
            0x21 => Self::Ds41IntersystemTransfer,
            0x22 => Self::UserZoneUpdateRequest,
            0x23 => Self::CallCancel,
            0x24 => Self::DeviceInformation,
            0x25 => Self::McmapInitialL3,
            0x26 => Self::McmapL3,
            0x27 => Self::RTmsiAssignmentCompletion,
            0x28 => Self::BsStatusRequest,
            0x29 => Self::CdmaOfftimeReport,
            0x2A => Self::AuthResync,
            0x2B => Self::AuthResponse,
            0x2C => Self::ItbspmRequest,
            0x2D => Self::HandoffSuppInfoNotification,
            0xFF => Self::GeneralExtension,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageId, WireChannel};

    #[test]
    fn sync_channel_wire_ids_match_c_s0004_table_3_1_2_3_1_1_2_1_sync_context() {
        assert_eq!(
            Some(0x01),
            MessageId::SyncChannelMessage.wire_type(WireChannel::Sync)
        );
        assert_eq!(
            Some(MessageId::SyncChannelMessage),
            MessageId::from_wire(WireChannel::Sync, 0x01)
        );
        assert_eq!(
            None,
            MessageId::SystemParameters.wire_type(WireChannel::Sync)
        );
    }

    #[test]
    fn forward_common_wire_ids_match_c_s0004_table_3_1_2_3_1_1_2_1() {
        let expected = [
            (MessageId::SystemParameters, 0x01),
            (MessageId::AccessParameters, 0x02),
            (MessageId::NeighborList, 0x03),
            (MessageId::CdmaChannelList, 0x04),
            (MessageId::Order, 0x07),
            (MessageId::ChannelAssignment, 0x08),
            (MessageId::DataBurst, 0x09),
            (MessageId::AuthChallenge, 0x0A),
            (MessageId::SsdUpdate, 0x0B),
            (MessageId::FeatureNotification, 0x0C),
            (MessageId::ExtSystemParameters, 0x0D),
            (MessageId::ExtNeighborList, 0x0E),
            (MessageId::StatusRequest, 0x0F),
            (MessageId::ServiceRedirection, 0x10),
            (MessageId::GeneralPage, 0x11),
            (MessageId::GlobalServiceRedirection, 0x12),
            (MessageId::TmsiAssignment, 0x13),
            (MessageId::Paca, 0x14),
            (MessageId::ExtChannelAssignment, 0x15),
            (MessageId::GeneralNeighborList, 0x16),
            (MessageId::UserZoneIdentification, 0x17),
            (MessageId::PrivateNeighborList, 0x18),
            (MessageId::ExtGlobalServiceRedirection, 0x1A),
            (MessageId::ExtCdmaChannelList, 0x1B),
            (MessageId::UserZoneReject, 0x1C),
            (MessageId::Ansi41SystemParameters, 0x1D),
            (MessageId::McRrParameters, 0x1E),
            (MessageId::Ansi41Rand, 0x1F),
            (MessageId::EnhancedAccessParameters, 0x20),
            (MessageId::UniversalNeighborList, 0x21),
            (MessageId::SecurityModeCommand, 0x22),
            (MessageId::UniversalPage, 0x23),
            (MessageId::UniversalPageFirstSegment, 0x24),
            (MessageId::UniversalPageMiddleSegment, 0x25),
            (MessageId::UniversalPageFinalSegment, 0x26),
            (MessageId::McMapSyncChannel, 0x27),
            (MessageId::McMapSystemInformation, 0x28),
            (MessageId::McmapL3, 0x29),
            (MessageId::RTmsiAssignment, 0x2A),
            (MessageId::McMapFlowRelease, 0x2B),
            (MessageId::AuthenticationRequest, 0x2C),
            (MessageId::BroadcastServiceParameters, 0x2D),
            (MessageId::MeidExtChannelAssignment, 0x2E),
            (MessageId::AlternativeTechnologiesInformation, 0x2F),
            (MessageId::AccessPointIdentifier, 0x30),
            (MessageId::AccessPointIdentifierText, 0x31),
            (MessageId::AccessPointPilotInformation, 0x32),
            (MessageId::GeneralOverheadInformation, 0x33),
            (MessageId::FlexDuplexCdmaChannelList, 0x34),
            (MessageId::GeneralExtension, 0x3F),
        ];

        for (id, raw) in expected {
            assert_eq!(Some(raw), id.wire_type(WireChannel::ForwardCommon));
            assert_eq!(
                Some(id),
                MessageId::from_wire(WireChannel::ForwardCommon, raw)
            );
        }

        assert_eq!(
            None,
            MessageId::SyncChannelMessage.wire_type(WireChannel::ForwardCommon)
        );

        for reserved in [0x05, 0x06, 0x19, 0x35, 0x3E] {
            assert_eq!(
                None,
                MessageId::from_wire(WireChannel::ForwardCommon, reserved)
            );
        }
    }

    #[test]
    fn reverse_common_wire_ids_match_c_s0004_table_2_1_1_4_1_1_2_1() {
        let expected = [
            (MessageId::Registration, 0x01),
            (MessageId::Order, 0x02),
            (MessageId::DataBurst, 0x03),
            (MessageId::Origination, 0x04),
            (MessageId::PageResponse, 0x05),
            (MessageId::AuthChallengeResponse, 0x06),
            (MessageId::StatusResponse, 0x07),
            (MessageId::TmsiAssignmentCompletion, 0x08),
            (MessageId::PacaCancel, 0x09),
            (MessageId::ExtStatusResponse, 0x0A),
            (MessageId::DeviceInformation, 0x0D),
            (MessageId::SecurityModeRequest, 0x0E),
            (MessageId::AuthResponse, 0x15),
            (MessageId::AuthResync, 0x16),
            (MessageId::Reconnect, 0x17),
            (MessageId::RadioEnvironment, 0x18),
            (MessageId::CallRecoveryRequest, 0x19),
            (MessageId::GeneralExtension, 0x3F),
        ];

        for (id, raw) in expected {
            assert_eq!(Some(raw), id.wire_type(WireChannel::ReverseCommon));
            assert_eq!(
                Some(id),
                MessageId::from_wire(WireChannel::ReverseCommon, raw)
            );
        }

        for reserved in [0x0B, 0x0C, 0x0F, 0x10, 0x11, 0x12] {
            assert_eq!(
                None,
                MessageId::from_wire(WireChannel::ReverseCommon, reserved)
            );
        }
    }

    #[test]
    fn reverse_common_spec_tags_match_message_names() {
        assert_eq!("PACNM", MessageId::PacaCancel.tag());
        assert_eq!("RCNM", MessageId::Reconnect.tag());
    }

    #[test]
    fn forward_common_spec_tags_match_message_names() {
        assert_eq!("AUCM", MessageId::AuthChallenge.tag());
        assert_eq!("STRQM", MessageId::StatusRequest.tag());
        assert_eq!("TASM", MessageId::TmsiAssignment.tag());
        assert_eq!("UZIM", MessageId::UserZoneIdentification.tag());
        assert_eq!("MECAM", MessageId::MeidExtChannelAssignment.tag());
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
