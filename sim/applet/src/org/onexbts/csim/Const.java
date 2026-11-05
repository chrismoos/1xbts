package org.onexbts.csim;

/**
 * File IDs, INS codes, SW values, access conditions.
 * Spec refs: C.S0065-B (CSIM on UICC), ETSI TS 102 221 (UICC-Terminal).
 */
public final class Const {

    private Const() {}

    // CLA: TS 102 221 §10.1.1. 0X and 4X are the interindustry commands,
    // 8X is the CSIM commands of C.S0065-B Table 1 and this applet's
    // personalization commands. Secure messaging is not supported, so
    // CLA values with SM bits set are rejected.
    public static final byte CLA_MASK_LC      = (byte) 0xFC;
    public static final byte CLA_BASE         = (byte) 0x00;
    public static final byte CLA_MASK_INTER1  = (byte) 0xF0;
    public static final byte CLA_INTER1       = (byte) 0x40;
    public static final byte CLA_PROP_CSIM    = (byte) 0x80;
    public static final byte CLA_GSM          = (byte) 0xA0;

    // INS, TS 102 221 §10.1.2 (CLA 0X / 4X)
    public static final byte INS_SELECT             = (byte) 0xA4;
    public static final byte INS_STATUS             = (byte) 0xF2;
    public static final byte INS_READ_BINARY        = (byte) 0xB0;
    public static final byte INS_UPDATE_BINARY      = (byte) 0xD6;
    public static final byte INS_READ_RECORD        = (byte) 0xB2;
    public static final byte INS_UPDATE_RECORD      = (byte) 0xDC;
    public static final byte INS_SEARCH_RECORD      = (byte) 0xA2;
    public static final byte INS_INCREASE           = (byte) 0x32;
    public static final byte INS_VERIFY_PIN         = (byte) 0x20;
    public static final byte INS_CHANGE_PIN         = (byte) 0x24;
    public static final byte INS_DISABLE_PIN        = (byte) 0x26;
    public static final byte INS_ENABLE_PIN         = (byte) 0x28;
    public static final byte INS_UNBLOCK_PIN        = (byte) 0x2C;
    public static final byte INS_DEACTIVATE_FILE    = (byte) 0x04;
    public static final byte INS_ACTIVATE_FILE      = (byte) 0x44;
    public static final byte INS_AUTHENTICATE       = (byte) 0x88;
    public static final byte INS_MANAGE_CHANNEL     = (byte) 0x70;
    public static final byte INS_GET_RESPONSE       = (byte) 0xC0;
    public static final byte INS_TERMINAL_PROFILE   = (byte) 0x10;
    public static final byte INS_TERMINAL_CAPABILITY= (byte) 0xAA;

    // INS, C.S0065-B Table 1 (CLA 8X)
    public static final byte INS_MANAGE_SSD         = (byte) 0x82;
    public static final byte INS_BASE_STATION_CHAL  = (byte) 0x8A;
    public static final byte INS_GENERATE_KEY_VPM   = (byte) 0x8E;
    public static final byte INS_KEY_REQUEST        = (byte) 0x50;
    public static final byte INS_COMMIT             = (byte) 0xCC;
    public static final byte INS_VALIDATE           = (byte) 0xCE;
    public static final byte INS_CONFIG_REQUEST     = (byte) 0x54;
    public static final byte INS_DOWNLOAD_REQUEST   = (byte) 0x56;
    public static final byte INS_OTAPA_REQUEST      = (byte) 0xEE;
    public static final byte INS_SECURE_MODE        = (byte) 0x4A;
    public static final byte INS_FRESH              = (byte) 0x4C;
    public static final byte INS_STORE_ESN_MEID_ME  = (byte) 0xDE;
    public static final byte INS_COMPUTE_IP_AUTH    = (byte) 0x80;
    public static final byte INS_BCMCS              = (byte) 0x58;
    public static final byte INS_APP_AUTH           = (byte) 0x5A;
    public static final byte INS_UMAC_GENERATION    = (byte) 0x5E;
    public static final byte INS_CONFIRM_KEYS       = (byte) 0x5C;
    public static final byte INS_SSAFE_VERIFY       = (byte) 0x40;
    public static final byte INS_TLS_MASTER_SECRET  = (byte) 0x42;
    public static final byte INS_TLS_VERIFY_DATA    = (byte) 0x44;
    public static final byte INS_TLS_KEY_BLOCK      = (byte) 0x46;

    // INS, this applet's personalization commands (CLA 8X)
    // Writes a transparent EF regardless of its UPDATE condition. P1P2 =
    // FID. ADM1 required. For the EFs whose UPDATE condition is NEVER
    // and whose value is fixed at pre-personalization.
    public static final byte INS_PREPERSO_UPDATE    = (byte) 0xF4;
    // Loads CAVE secret state (A-key / SSD) that is never an EF. P1 picks
    // which secret. ADM1 required. See Csim.doLoadCaveSecret.
    public static final byte INS_LOAD_CAVE          = (byte) 0xF6;
    // Computes the 6-digit A-key checksum (S.S0053 §2.2.1) over the stored
    // A-key and the ESN in the data field. ADM1 required, provisioning aid.
    public static final byte INS_AKEY_CHECKSUM      = (byte) 0xF8;
    // Provisions a packet-data shared secret. P1 = class (CHAP/MN-HA/
    // MN-AAA/HRPD), P2 = NAI-Entry-Index. ADM1 required.
    public static final byte INS_LOAD_IPSEC         = (byte) 0xF0;
    // Loads the AKA root key K (16 bytes) and OPc (16 bytes). ADM1.
    public static final byte INS_LOAD_AKA_KEY       = (byte) 0xEA;
    // Loads EF_META. P1 b8 set on the last chunk, P1 b7..b1 || P2 =
    // offset. ADM1 required.
    public static final byte INS_INIT_META          = (byte) 0xFA;
    // Sets PIN1 or PIN2 (P1 = 00) or PUK1 or PUK2 (P1 = 01), with P2 = 01
    // or 81, and resets its retry counter. ADM1 required.
    public static final byte INS_SET_PIN            = (byte) 0xEC;
    // Debug: dump EF_META (P1P2 = offset), dump and clear the APDU trace
    // ring buffer, report platform limits. A hardened build answers 6D00
    // to all but PLATFORM_INFO, which requires ADM1 there.
    public static final byte INS_DEBUG_DUMP_META    = (byte) 0xFB;
    public static final byte INS_DEBUG_DUMP_APDULOG = (byte) 0xFC;
    public static final byte INS_DEBUG_CLEAR_APDULOG= (byte) 0xFD;
    public static final byte INS_DEBUG_PLATFORM_INFO= (byte) 0xFE;
    // Debug: S.S0055 f0/f1/f3 and ESP self-test on caller-supplied inputs.
    public static final byte INS_DEBUG_CRYPTO_SELFTEST = (byte) 0xE6;

    // File IDs
    public static final short FID_MF                = (short) 0x3F00;
    public static final short FID_ADF               = (short) 0x7FFF;
    public static final short FID_EF_DIR             = (short) 0x2F00;
    public static final short FID_EF_ICCID           = (short) 0x2FE2;
    public static final short FID_EF_PL              = (short) 0x2F05;
    public static final short FID_EF_COUNT           = (short) 0x6F21;
    public static final short FID_EF_IMSI_M          = (short) 0x6F22;
    public static final short FID_EF_IMSI_T          = (short) 0x6F23;
    public static final short FID_EF_TMSI            = (short) 0x6F24;
    public static final short FID_EF_RUIMID          = (short) 0x6F31;
    public static final short FID_EF_SPC             = (short) 0x6F33;
    public static final short FID_EF_ESN_ME          = (short) 0x6F38;
    public static final short FID_EF_USGIND          = (short) 0x6F42;
    public static final short FID_EF_SPCS            = (short) 0x6F46;
    public static final short FID_EF_SF_EUIMID       = (short) 0x6F74;
    public static final short FID_EF_3GCIK           = (short) 0x6F6B;
    public static final short FID_EF_OTAPASPC        = (short) 0x6F34;
    public static final short FID_EF_MDN             = (short) 0x6F44;
    public static final short FID_EF_EPRL            = (short) 0x6F5A;
    public static final short FID_EF_MAXPRL          = (short) 0x6F45;
    public static final short FID_EF_3GPDOPM         = (short) 0x6F49;
    public static final short FID_EF_PUZL            = (short) 0x6F53;
    public static final short FID_EF_MMSUP           = (short) 0x6F68;
    public static final short FID_EF_HOME_TAG        = (short) 0x6F70;
    public static final short FID_EF_LI              = (short) 0x6F3A;
    public static final short FID_EF_FDN             = (short) 0x6F3B;
    public static final short FID_EF_SMS             = (short) 0x6F3C;
    public static final short FID_EF_SMSP            = (short) 0x6F3D;
    public static final short FID_EF_SMSS            = (short) 0x6F3E;
    public static final short FID_EF_SPN             = (short) 0x6F41;
    public static final short FID_EF_ECC             = (short) 0x6F47;
    public static final short FID_EF_CSSPR           = (short) 0x6F58;
    public static final short FID_EF_BCSMSPREF       = (short) 0x6F5C;
    public static final short FID_EF_BCSMSTABLE      = (short) 0x6F5D;
    public static final short FID_EF_BCSMSP          = (short) 0x6F5E;
    public static final short FID_EF_MMSN            = (short) 0x6F65;
    public static final short FID_EF_EXT8            = (short) 0x6F66;
    public static final short FID_EF_MMSICP          = (short) 0x6F67;
    public static final short FID_EF_MMSUCP          = (short) 0x6F69;
    public static final short FID_EF_GID1            = (short) 0x6F6D;
    public static final short FID_EF_GID2            = (short) 0x6F6E;
    public static final short FID_EF_GROUP_TAG       = (short) 0x6F71;
    public static final short FID_EF_SPECIFIC_TAG    = (short) 0x6F72;
    public static final short FID_EF_CALL_PROMPT     = (short) 0x6F73;
    public static final short FID_EF_SDN             = (short) 0x6F79;
    public static final short FID_EF_EXT2            = (short) 0x6F7A;
    public static final short FID_EF_EXT3            = (short) 0x6F7B;
    public static final short FID_EF_ICI             = (short) 0x6F7C;
    public static final short FID_EF_OCI             = (short) 0x6F7D;
    public static final short FID_EF_EXT5            = (short) 0x6F7E;
    public static final short FID_EF_CCP2            = (short) 0x6F7F;
    public static final short FID_EF_MODEL           = (short) 0x6F81;
    public static final short FID_EF_WAPBROWSER_BM   = (short) 0x6F8B;
    public static final short FID_EF_ACCOLC          = (short) 0x6F2C;

    // File structure types (TS 102 221 §11.1.1.4.3)
    public static final byte FS_TRANSPARENT  = (byte) 0x01;
    public static final byte FS_LINEAR_FIXED = (byte) 0x02;
    public static final byte FS_CYCLIC       = (byte) 0x06;

    // Access conditions
    public static final byte AC_ALWAYS = 0;
    public static final byte AC_PIN1   = 1;
    public static final byte AC_ADM    = 2;
    public static final byte AC_NEVER  = 3;
    public static final byte AC_PIN2   = 4;

    // Status words (TS 102 221 §10.2.1)
    public static final short SW_FILE_DEACTIVATED     = (short) 0x6283;
    public static final short SW_INCOMPATIBLE_FILE    = (short) 0x6981;
    public static final short SW_SECURITY_NOT_SAT     = (short) 0x6982;
    public static final short SW_AUTH_METHOD_BLOCKED  = (short) 0x6983;
    public static final short SW_REF_DATA_INVALIDATED = (short) 0x6984;
    public static final short SW_CONDITIONS_NOT_SAT   = (short) 0x6985;
    public static final short SW_NO_EF_SELECTED       = (short) 0x6986;
    public static final short SW_WRONG_DATA           = (short) 0x6A80;
    public static final short SW_FUNCTION_NOT_SUPPORTED = (short) 0x6A81;
    public static final short SW_FILE_NOT_FOUND       = (short) 0x6A82;
    public static final short SW_RECORD_NOT_FOUND     = (short) 0x6A83;
    public static final short SW_WRONG_PARAMS_P1P2    = (short) 0x6A86;
    public static final short SW_REF_DATA_NOT_FOUND   = (short) 0x6A88;
    public static final short SW_INCORRECT_P1P2       = (short) 0x6B00;
    public static final short SW_CHANNEL_NOT_SUPPORTED= (short) 0x6881;
    public static final short SW_INCREASE_MAX_VALUE   = (short) 0x9850;
    public static final short SW_VERIFY_FAILED_BASE   = (short) 0x63C0;

    // CSIM security command status words (C.S0023-D §4.4)
    // Generate Key/VPM before a Run CAVE with save registers on, or an
    // out-of-sequence Manage SSD command (C.S0023-D §4.3.2.4, §4.4.5).
    public static final short SW_CAVE_NOT_SAVED       = (short) 0x9834;
    // Confirm SSD AUTHBS mismatch (C.S0023-D §4.4.3).
    public static final short SW_AUTH_COMPARE_FAIL    = (short) 0x9804;

    // PIN policy (TS 102 221 §9.4)
    public static final byte PIN_MAX_TRIES = 3;
    public static final byte PIN_MAX_SIZE  = 8;
    public static final byte PUK_MAX_TRIES = 10;
    public static final byte PUK_MAX_SIZE  = 8;
    public static final byte ADM_MAX_TRIES = 10;
    public static final byte ADM_MAX_SIZE  = 8;

    // Factory PIN1, PIN2, PUK1, PUK2 and ADM1. Personalization must change them.
    public static final byte[] DEFAULT_PIN1 = {
        (byte)'1', (byte)'2', (byte)'3', (byte)'4',
        (byte)0xFF, (byte)0xFF, (byte)0xFF, (byte)0xFF
    };
    public static final byte[] DEFAULT_PIN2 = {
        (byte)'2', (byte)'2', (byte)'2', (byte)'2',
        (byte)0xFF, (byte)0xFF, (byte)0xFF, (byte)0xFF
    };
    public static final byte[] DEFAULT_PUK1 = {
        (byte)'1', (byte)'2', (byte)'3', (byte)'4',
        (byte)'5', (byte)'6', (byte)'7', (byte)'8'
    };
    public static final byte[] DEFAULT_PUK2 = {
        (byte)'8', (byte)'7', (byte)'6', (byte)'5',
        (byte)'4', (byte)'3', (byte)'2', (byte)'1'
    };
    public static final byte[] DEFAULT_ADM1 = {
        0, 0, 0, 0, 0, 0, 0, 0
    };
}
