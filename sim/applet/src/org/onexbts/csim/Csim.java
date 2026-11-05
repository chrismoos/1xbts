package org.onexbts.csim;

import javacard.framework.APDU;
import javacard.framework.CardRuntimeException;
import javacard.framework.Applet;
import javacard.framework.ISO7816;
import javacard.framework.ISOException;
import javacard.framework.JCSystem;
import javacard.framework.MultiSelectable;
import javacard.framework.Util;
import javacard.framework.OwnerPIN;
import javacard.security.RandomData;

/**
 * CSIM applet (C.S0065-B on a UICC per ETSI TS 102 221).
 *
 * Implemented: the §5.2 file system and the TS 102 221 file, PIN and
 * channel commands, CAVE authentication (§9.4.1), OTASP/OTAPA (§9.4.2),
 * AKA and UMAC (§9.4.1.4, §9.4.7), Compute IP Authentication (§9.4.4),
 * and this applet's personalization commands.
 *
 * Not implemented: BCMCS, Application Authentication, LCS and TLS (6D00).
 * The S.S0055 functions are an int-free stub (6A81), which disables AKA,
 * Secure Mode and the OTASP key generation that uses them. The OTASP
 * A-key exchange needs an RSA engine.
 */
public final class Csim extends Applet implements MultiSelectable {

    // Key references, TS 102 221 §9.4.2 Table 9.3.
    private static final byte P2_PIN1 = (byte) 0x01;
    private static final byte P2_PIN2 = (byte) 0x81;
    private static final byte P2_ADM1 = (byte) 0x0A;

    // RID A000000343 + PIX 1002 (C.S0065-B §4) + issuer bytes.
    static final byte[] CSIM_AID = {
        (byte) 0xA0, (byte) 0x00, (byte) 0x00, (byte) 0x03, (byte) 0x43,
        (byte) 0x10, (byte) 0x02,
        (byte) 0xF3, (byte) 0x10, (byte) 0xFF, (byte) 0xFF,
        (byte) 0x89, (byte) 0x02,
        (byte) 0x00, (byte) 0x00, (byte) 0xFF
    };
    private static final short AID_MIN_PARTIAL = 5;

    // SELECT P1 (TS 102 221 §11.1.1.2).
    private static final byte SEL_BY_FID = 0x00;
    private static final byte SEL_CHILD_DF = 0x01;
    private static final byte SEL_PARENT_DF = 0x03;
    private static final byte SEL_BY_DF_NAME = 0x04;
    private static final byte SEL_PATH_FROM_MF = 0x08;
    private static final byte SEL_PATH_FROM_DF = 0x09;
    private static final byte SEL_P2_NO_DATA = 0x0C;
    private static final byte SEL_P2_DATA_MASK = 0x0C;

    // Short file identifier (TS 102 221 §8.4.2). In P1 of READ / UPDATE
    // BINARY and INCREASE, b8 b7 b6 = 100 selects by SFI in b5..b1. In P2 of
    // the record commands and in FCP tag 88 the SFI is b8..b4.
    private static final byte SFI_MASK = 0x1F;
    private static final byte SFI_SHIFT = 3;
    private static final byte P1_SFI_FLAG = (byte) 0x80;
    private static final byte P1_SFI_RFU = 0x60;
    private static final byte P1_SFI_PREFIX_MASK = (byte) 0xE0;

    // STATUS P2 (TS 102 221 §11.1.2).
    private static final byte STATUS_P2_DF_NAME = 0x01;
    private static final byte STATUS_P2_NO_DATA = 0x0C;

    // Record addressing modes, P2 b3..b1 (TS 102 221 §11.1.5 / §11.1.6).
    private static final byte REC_MODE_MASK = 0x07;
    private static final byte REC_NEXT = 0x02;
    private static final byte REC_PREVIOUS = 0x03;
    private static final byte REC_ABSOLUTE = 0x04;
    private static final byte SEARCH_FORWARD = 0x04;
    private static final byte SEARCH_BACKWARD = 0x05;

    // FCP tags (TS 102 221 §11.1.1.3).
    private static final byte TAG_FCP = 0x62;
    private static final byte TAG_FILE_SIZE = (byte) 0x80;
    private static final byte TAG_FILE_DESCRIPTOR = (byte) 0x82;
    private static final byte TAG_FILE_ID = (byte) 0x83;
    private static final byte TAG_DF_NAME = (byte) 0x84;
    private static final byte TAG_SFI = (byte) 0x88;
    private static final byte TAG_LIFE_CYCLE = (byte) 0x8A;
    private static final byte TAG_PROPRIETARY = (byte) 0xA5;
    private static final byte TAG_UICC_CHARACTERISTICS = (byte) 0x80;
    private static final byte TAG_PIN_STATUS_TEMPLATE = (byte) 0xC6;
    private static final byte TAG_PS_DO = (byte) 0x90;
    private static final byte TAG_KEY_REF = (byte) 0x83;
    // File descriptor byte: b7 shareable, b6..b4 000 working EF / 111 DF.
    private static final byte FD_DF = (byte) 0x78;
    private static final byte FD_EF_SHAREABLE = (byte) 0x40;
    private static final byte DATA_CODING = 0x21;
    private static final byte LCS_ACTIVATED = 0x05;
    private static final byte LCS_DEACTIVATED = 0x04;
    // UICC characteristics: clock stop allowed, voltage classes A, B, C.
    private static final byte UICC_CHARACTERISTICS = 0x71;
    // Security attributes in the expanded format (TS 102 221 §9.2.6,
    // ISO/IEC 7816-4 §5.4.3.3): AM_DO tag 80 holds an AM byte, tag 84 an
    // INS code. SC_DO 90 = always, 97 = never, A4 = a key reference that
    // must be verified. The expanded format names the key reference, so
    // PIN1, PIN2 and ADM1 are distinguishable in the FCP, and it is the
    // only format that can state the INCREASE condition.
    private static final byte TAG_SEC_EXPANDED = (byte) 0xAB;
    private static final byte AM_DO_BYTE = (byte) 0x80;
    private static final byte AM_DO_INS = (byte) 0x84;
    private static final byte AM_READ = 0x01;
    private static final byte AM_UPDATE = 0x02;
    private static final byte AM_DEACTIVATE = 0x08;
    private static final byte AM_ACTIVATE = 0x10;
    private static final byte AM_DF_DELETE = 0x40;
    private static final byte SC_DO_ALWAYS = (byte) 0x90;
    private static final byte SC_DO_NEVER = (byte) 0x97;
    private static final byte SC_DO_AUTH = (byte) 0xA4;
    private static final byte TAG_USAGE_QUALIFIER = (byte) 0x95;
    private static final byte USAGE_PIN_VERIFICATION = 0x08;

    // Store ESN_MEID_ME (C.S0065-B §9.4.3.1).
    private static final byte ESN_P1_ESN = 0x00;
    private static final byte ESN_P1_MEID = 0x01;
    private static final short ESN_ME_LEN = 8;
    private static final byte ESN_LEN = 4;
    private static final byte MEID_LEN = 7;
    private static final byte ESN_RESP_CHANGED = 0x01;
    private static final byte ESN_RESP_UIM_ID = 0x10;
    private static final byte ESN_RESP_SF_EUIMID = 0x20;
    private static final byte USGIND_UIM_ID = 0x01;
    private static final byte USGIND_SF_EUIMID = 0x02;

    // CAVE commands (C.S0065-B §9.4.1, data layouts from C.S0023-D §4.4).
    private static final byte AUTH_P2_RUN_CAVE = (byte) 0x80;
    private static final byte AUTH_P2_3G_AKA = (byte) 0x81;
    private static final byte AUTH_P2_WLAN_AKA = (byte) 0x82;
    private static final byte RANDTYPE_RAND = 0x00;
    private static final byte RANDTYPE_RANDU = 0x01;
    private static final byte SSD_P2_UPDATE = 0x00;
    private static final byte SSD_P2_CONFIRM = 0x01;
    private static final byte PC_COMMIT_TRIGGER = 0x04;  // Process_Control bit 2
    private static final byte PC_SAVE_REGS = 0x10;       // Process_Control bit 4
    private static final byte CAVE_LOAD_AKEY = 0x00;
    private static final byte CAVE_LOAD_SSD = 0x01;
    private static final short CAVE_AKEY_LEN = 8;
    private static final short CAVE_SSD_LEN = 16;     // SSD_A || SSD_B
    private static final short RUN_CAVE_LC = 17;
    private static final short UPDATE_SSD_LC = 15;
    private static final short CONFIRM_SSD_LC = 3;
    private static final short BS_CHALLENGE_LC = 4;
    private static final short GEN_KEY_LC = 2;
    private static final short AKEY_CSUM_DIGITS = 6;
    // Compute IP Authentication P1 (C.S0023-D §4.8.1).
    private static final byte IP_P1_CHAP = 0x00;
    private static final byte IP_P1_MN_HA = 0x01;
    private static final byte IP_P1_MIP_RRQ = 0x02;
    private static final byte IP_P1_MN_AAA = 0x03;
    private static final byte IP_P1_HRPD = 0x04;
    // OTASP (C.S0065-B §9.4.2, data from C.S0023-D §4.5, C.S0016-C §4.5.4).
    private static final byte KEY_P2_MS_KEY = 0x00;
    private static final byte KEY_P2_KEY_GEN = 0x01;
    private static final byte KEY_P2_SERVICE = 0x02;
    private static final byte VAL_BLOCK_VERIFY_SPC = 0x00;
    private static final byte VAL_BLOCK_CHANGE_SPC = 0x01;
    private static final byte VAL_BLOCK_SPASM = 0x02;
    private static final byte VAL_RESULT_ACCEPT = 0x00;
    private static final byte VAL_RESULT_REJECT = 0x01;
    // C.S0016-C §4.5.4 RESULT_CODE values.
    private static final byte VAL_RESULT_LOCKED = 0x0A;
    private static final byte VAL_RESULT_INVALID_SPC = 0x0B;
    private static final byte VAL_RESULT_INVALID_SPASM = 0x0D;
    private static final short SPC_LEN = 3;
    // Failed Verify SPC attempts before the SPC is refused until an ADM1
    // write of EF_SPC.
    private static final byte SPC_MAX_TRIES = 10;
    private static final short OTA_SPC_VERIFIED = 0;
    private static final short OTA_OTAPA_ACTIVE = 1;
    private static final short OTA_PRL_PENDING = 2;
    // NAM_LOCK_STATE of C.S0016-C §3.2.2: set when an OTAPA session opens on
    // a SPASM-protected NAM, cleared by a matching Validate SPASM.
    private static final short OTA_NAM_LOCKED = 3;
    private static final short OTA_FLAGS_LEN = 4;
    private static final byte OTAPA_START = 0x01;
    private static final short OTAPA_RESP_WITH_RAND = 6;
    private static final short OTAPA_RESP_NO_RAND = 2;
    // OTASP download staging: a small table of blocks held for Commit.
    // Each slot is [valid, block id, length, data...].
    private static final short STAGE_SLOTS = 4;
    private static final short STAGE_SLOT_DATA = 48;
    private static final short STAGE_HDR = 3;
    private static final short STAGE_SLOT = (short) (STAGE_HDR + STAGE_SLOT_DATA);
    // Generic Config/Download families (C.S0065-B Tables 5/6).
    private static final byte DL_P2_NAM = 0x00;
    private static final byte CFG_P2_NAM = 0x00;
    private static final byte CFG_P2_SSPR = 0x01;
    private static final byte CFG_P2_PUZL = 0x02;
    private static final byte CFG_P2_3GPD = 0x03;
    private static final byte CFG_P2_MMS = 0x04;
    private static final byte CFG_P2_SYSTAG = 0x05;
    private static final byte SM_P1_START = 0x00;
    private static final byte FRESH_P1_PUT = 0x00;
    private static final byte FRESH_P1_GET = 0x01;
    // Chaining P2 for MN-HA / MIP-RRQ.
    private static final byte IP_BLK_FIRST = 0x00;
    private static final byte IP_BLK_NEXT = 0x01;
    private static final byte IP_BLK_SINGLE = 0x02;
    private static final byte IP_BLK_LAST = 0x03;
    private static final short IMSI_M_LEN = 10;
    // Offsets into `scratch` used by the CAVE handlers (disjoint regions).
    private static final short SC_ESN = 0;
    private static final short SC_AUTHDATA = 4;
    private static final short SC_IMSI = 8;
    private static final short SC_RAND = 20;

    private static final short INCREASE_MAX_DATA_LEN = 127;
    private static final short READ_BINARY_MAX = 256;
    private static final short PIN_STATUS_QUERY_LEN = 0;

    private static final byte DF_MF = 0;
    private static final byte DF_ADF = 1;

    private final FileSystem fs;
    private final Ac ac;
    private Cave cave;
    private IpAuth ipAuth;
    private Aka aka;
    private OtaspDh otaspDh;
    private OtaNam otaNam;
    private Ss0055 ss0055;

    // Built on first use by the accessors below, never in the constructor
    // (see the warning there). buildStage and buildReason record the last
    // subsystem whose constructor threw, for PLATFORM_INFO.
    private byte buildStage;
    private short buildReason;
    private static final byte STAGE_CAVE = 1;
    private static final byte STAGE_IPAUTH = 2;
    private static final byte STAGE_SS0055 = 3;
    private static final byte STAGE_AKA = 4;
    private static final byte STAGE_OTASPDH = 5;
    private static final byte STAGE_OTANAM = 6;
    private byte[] otaFlags;        // transient: see OTA_* indices
    private byte[] otaAuthOtapa;    // transient: 3-byte AUTH_OTAPA of the open OTAPA session
    // OTASP download staging: [0] valid, [1] block id, [2] length, [3..] data.
    private byte[] stage;
    private byte[] fresh;           // transient: [0] valid, [1..2] Crypto-Sync
    private byte[] secMode;         // transient: [0] secure mode active
    private RandomData rng;
    private final byte[] scratch;       // transient, FCP and INCREASE work area
    private static final short SCRATCH_LEN = 128;
    private final byte[] esnScratch;    // transient, Store ESN_MEID_ME compare
    private final byte[] dfOfChannel;
    private final byte[] selectedRecord;      // 1-based, 0 = none
    private final short[] tmpOffset;

    // APDU trace ring buffer, 32 entries x 8 bytes:
    // [CLA, INS, P1, P2, LC, SW hi, SW lo, 0].
    private byte[] apduLog;             // allocated on the first APDU, not at install
    private short apduLogHead;
    // 32 entries of CLA INS P1 P2 Lc, the first four data bytes of a
    // SELECT (a FID, a 7FFF path or an AID prefix), and the status word.
    private static final short APDU_LOG_SIZE = 352;
    private static final short APDU_LOG_ENTRY = 11;
    private static final short APDU_LOG_DATA_OFF = 5;
    private static final short APDU_LOG_DATA_LEN = 4;
    private static final short APDU_LOG_SW_OFF = 9;
    // SW bytes of an entry whose command has not completed.
    private static final byte APDU_LOG_SW_PENDING = (byte) 0xFF;
    private static final short APDU_LOG_DUMP_MAX = 254;
    private short currentLogSlot;

    private short currentOff;
    // Persistent. Incremented before the SPC comparison, so removing power
    // after a mismatch still counts the attempt.
    private byte spcFails;

    // Keep this constructor small. The JCRE runs install() as one atomic
    // transaction, and every object and array allocated here is journaled
    // against the card's commit capacity (3316 bytes on the sysmoISIM-SJA5).
    // Exceeding it aborts the whole install, and INSTALL [for install and
    // make selectable] returns 6A80 after the CAP has loaded cleanly. A try
    // here cannot catch it. Allocate new persistent state lazily on first
    // use, as the security subsystems, EF_META, the EF pool and the APDU
    // trace are. Check a new allocation by installing on the card. jcardsim
    // has no commit limit and installs regardless.
    private Csim() {
        fs = new FileSystem(CSIM_AID);
        ac = new Ac();
        otaFlags = JCSystem.makeTransientByteArray(OTA_FLAGS_LEN, JCSystem.CLEAR_ON_DESELECT);
        otaAuthOtapa = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        stage = JCSystem.makeTransientByteArray((short) (STAGE_SLOTS * STAGE_SLOT), JCSystem.CLEAR_ON_DESELECT);
        fresh = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        secMode = JCSystem.makeTransientByteArray((short) 1, JCSystem.CLEAR_ON_DESELECT);
        scratch = JCSystem.makeTransientByteArray(SCRATCH_LEN,
                JCSystem.CLEAR_ON_DESELECT);
        esnScratch = JCSystem.makeTransientByteArray(ESN_ME_LEN,
                JCSystem.CLEAR_ON_DESELECT);
        tmpOffset = JCSystem.makeTransientShortArray((short) 1,
                JCSystem.CLEAR_ON_DESELECT);
        dfOfChannel = new byte[FileSystem.MAX_CHANNELS];
        selectedRecord = new byte[FileSystem.MAX_CHANNELS];
        apduLogHead = 0;
    }

    private Cave cave() {
        if (cave == null) {
            try {
                cave = new Cave();
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_CAVE, e.getReason());
            }
        }
        return cave;
    }

    private IpAuth ipAuth() {
        if (ipAuth == null) {
            try {
                ipAuth = new IpAuth();
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_IPAUTH, e.getReason());
            }
        }
        return ipAuth;
    }

    private Ss0055 ss0055() {
        if (ss0055 == null) {
            try {
                ss0055 = new Ss0055();
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_SS0055, e.getReason());
            }
        }
        return ss0055;
    }

    private Aka aka() {
        if (aka == null) {
            Ss0055 kernel = ss0055();
            try {
                aka = new Aka(kernel);
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_AKA, e.getReason());
            }
        }
        return aka;
    }

    private OtaspDh otaspDh() {
        if (otaspDh == null) {
            Ss0055 kernel = ss0055();
            try {
                otaspDh = new OtaspDh(kernel);
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_OTASPDH, e.getReason());
            }
        }
        return otaspDh;
    }

    private OtaNam otaNam() {
        if (otaNam == null) {
            try {
                otaNam = new OtaNam(fs);
            } catch (CardRuntimeException e) {
                buildFailed(STAGE_OTANAM, e.getReason());
            }
        }
        return otaNam;
    }

    private void buildFailed(byte stage, short reason) {
        buildStage = stage;
        buildReason = reason;
        ISOException.throwIt(Const.SW_FUNCTION_NOT_SUPPORTED);
    }

    public static void install(byte[] bArray, short bOffset, byte bLength) {
        Csim applet = new Csim();
        byte aidLen = (bLength > 0) ? bArray[bOffset] : 0;
        if (aidLen > 0) {
            applet.register(bArray, (short) (bOffset + 1), aidLen);
        } else {
            applet.register();
        }
    }

    public boolean select(boolean appInstAlreadyActive) {
        return true;
    }

    public void deselect(boolean appInstStillActive) {
        byte ch = JCSystem.getAssignedChannel();
        if (ch < FileSystem.MAX_CHANNELS) {
            resetChannel(ch);
        }
        if (!appInstStillActive) {
            deselect();
        }
    }

    /** TS 102 221 §11.1.9.1: PIN verification state is per application
     *  session and ends with it, together with the current EF. */
    public void deselect() {
        if (ac != null) ac.clearAllChannels();
        if (fs != null) {
            for (byte ch = 0; ch < FileSystem.MAX_CHANNELS; ch++) {
                resetChannel(ch);
            }
        }
    }

    private void resetChannel(byte ch) {
        fs.closeChannel(ch);
        ac.clearChannel(ch);
        dfOfChannel[ch] = DF_MF;
        selectedRecord[ch] = 0;
    }

    // APDU dispatch

    public void process(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        if (Variant.DEBUG && apduLog == null) apduLog = new byte[APDU_LOG_SIZE];
        boolean isDebugDump = (buf[ISO7816.OFFSET_INS] == Const.INS_DEBUG_DUMP_APDULOG)
                           || (buf[ISO7816.OFFSET_INS] == Const.INS_DEBUG_CLEAR_APDULOG);
        short logSlot = (!Variant.DEBUG || isDebugDump) ? (short) -1 : logApduHeader(buf);
        currentLogSlot = logSlot;

        try {
            byte channel = channelOf(apdu);
            if (selectingApplet()) {
                logApduData(buf, apdu.setIncomingAndReceive());
                dfOfChannel[channel] = DF_ADF;
                fs.select(channel, null);
                selectedRecord[channel] = 0;
                sendFcpForAdf(apdu, buf[ISO7816.OFFSET_P2]);
                if (!isDebugDump) logApduSw(logSlot, ISO7816.SW_NO_ERROR);
                return;
            }
            byte cla = buf[ISO7816.OFFSET_CLA];
            boolean proprietary = (byte) (cla & Const.CLA_MASK_LC) == Const.CLA_PROP_CSIM;
            boolean interindustry = (byte) (cla & Const.CLA_MASK_LC) == Const.CLA_BASE
                    || (byte) (cla & Const.CLA_MASK_LC) == Const.CLA_GSM
                    || (byte) (cla & Const.CLA_MASK_INTER1) == Const.CLA_INTER1;
            if (!proprietary && !interindustry) {
                ISOException.throwIt(ISO7816.SW_CLA_NOT_SUPPORTED);
            }
            byte ins = buf[ISO7816.OFFSET_INS];
            if (proprietary) {
                processProprietary(apdu, ins, channel);
            } else {
                processInterindustry(apdu, ins, channel);
            }
            if (!isDebugDump) logApduSw(logSlot, ISO7816.SW_NO_ERROR);
        } catch (ISOException e) {
            if (!isDebugDump) logApduSw(logSlot, e.getReason());
            throw e;
        }
    }

    /** CLA 0X / 4X: TS 102 221 §10.1.2 plus Authenticate (C.S0065-B Table 1).
     *  STATUS is dispatched here too, although the spec assigns it CLA 8X. */
    private void processInterindustry(APDU apdu, byte ins, byte channel) {
        switch (ins) {
            case Const.INS_SELECT:           doSelect(apdu, channel); break;
            case Const.INS_STATUS:           doStatus(apdu, channel); break;
            case Const.INS_READ_BINARY:      doReadBinary(apdu, channel); break;
            case Const.INS_UPDATE_BINARY:    doUpdateBinary(apdu, channel); break;
            case Const.INS_READ_RECORD:      doReadRecord(apdu, channel); break;
            case Const.INS_UPDATE_RECORD:    doUpdateRecord(apdu, channel); break;
            case Const.INS_SEARCH_RECORD:    doSearchRecord(apdu, channel); break;
            case Const.INS_INCREASE:         doIncrease(apdu, channel); break;
            case Const.INS_VERIFY_PIN:       doVerifyPin(apdu, channel); break;
            case Const.INS_CHANGE_PIN:       doChangePin(apdu); break;
            case Const.INS_ENABLE_PIN:       doEnablePin(apdu, true); break;
            case Const.INS_DISABLE_PIN:      doEnablePin(apdu, false); break;
            case Const.INS_UNBLOCK_PIN:      doUnblockPin(apdu, channel); break;
            case Const.INS_DEACTIVATE_FILE:  doActivateFile(apdu, channel, false); break;
            case Const.INS_ACTIVATE_FILE:    doActivateFile(apdu, channel, true); break;
            case Const.INS_MANAGE_CHANNEL:   doManageChannel(apdu); break;
            case Const.INS_AUTHENTICATE:     doAuthenticate(apdu, channel); break;
            case Const.INS_GET_RESPONSE:     ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT); break;
            case Const.INS_TERMINAL_PROFILE:
            case Const.INS_TERMINAL_CAPABILITY:
                apdu.setIncomingAndReceive();
                break;
            default:
                ISOException.throwIt(ISO7816.SW_INS_NOT_SUPPORTED);
        }
    }

    /** CLA 8X: STATUS (TS 102 221 Table 10.5), C.S0065-B Table 1 and this
     *  applet's personalization commands. */
    private void processProprietary(APDU apdu, byte ins, byte channel) {
        switch (ins) {
            case Const.INS_STATUS:             doStatus(apdu, channel); break;
            case Const.INS_STORE_ESN_MEID_ME:  doStoreEsnMeidMe(apdu, channel); break;
            case Const.INS_MANAGE_SSD:         doManageSsd(apdu, channel); break;
            case Const.INS_BASE_STATION_CHAL:  doBaseStationChallenge(apdu, channel); break;
            case Const.INS_GENERATE_KEY_VPM:   doGenerateKeyVpm(apdu, channel); break;
            case Const.INS_LOAD_CAVE:          doLoadCaveSecret(apdu, channel); break;
            case Const.INS_AKEY_CHECKSUM:      doAKeyChecksum(apdu, channel); break;
            case Const.INS_COMPUTE_IP_AUTH:    doComputeIpAuth(apdu, channel); break;
            case Const.INS_LOAD_IPSEC:         doLoadIpSecret(apdu, channel); break;
            case Const.INS_LOAD_AKA_KEY:       doLoadAkaKey(apdu, channel); break;
            case Const.INS_DEBUG_CRYPTO_SELFTEST: requireDebug(); doCryptoSelfTest(apdu); break;
            case Const.INS_UMAC_GENERATION:    doUmacGeneration(apdu, channel); break;
            case Const.INS_CONFIRM_KEYS:       doConfirmKeys(apdu, channel); break;
            case Const.INS_KEY_REQUEST:        doKeyRequest(apdu, channel); break;
            case Const.INS_COMMIT:             doCommit(apdu, channel); break;
            case Const.INS_VALIDATE:           doValidate(apdu, channel); break;
            case Const.INS_OTAPA_REQUEST:      doOtapaRequest(apdu, channel); break;
            case Const.INS_CONFIG_REQUEST:     doConfigRequest(apdu, channel); break;
            case Const.INS_DOWNLOAD_REQUEST:   doDownloadRequest(apdu, channel); break;
            case Const.INS_SECURE_MODE:        doSecureMode(apdu, channel); break;
            case Const.INS_FRESH:              doFresh(apdu, channel); break;
            case Const.INS_PREPERSO_UPDATE:    doPrepersoUpdate(apdu, channel); break;
            case Const.INS_INIT_META:          doInitMeta(apdu, channel); break;
            case Const.INS_SET_PIN:            doSetPin(apdu, channel); break;
            case Const.INS_DEBUG_DUMP_META:    requireDebug(); doDebugDumpMeta(apdu); break;
            case Const.INS_DEBUG_DUMP_APDULOG: requireDebug(); doDebugDumpApduLog(apdu); break;
            case Const.INS_DEBUG_CLEAR_APDULOG: requireDebug(); doDebugClearApduLog(apdu); break;
            case Const.INS_DEBUG_PLATFORM_INFO: doDebugPlatformInfo(apdu, channel); break;
            default:
                ISOException.throwIt(ISO7816.SW_INS_NOT_SUPPORTED);
        }
    }

    /** The debug commands answer 6D00 in a hardened build. */
    private static void requireDebug() {
        if (!Variant.DEBUG) ISOException.throwIt(ISO7816.SW_INS_NOT_SUPPORTED);
    }

    private short logApduHeader(byte[] buf) {
        short slot = apduLogHead;
        apduLog[(short) (slot + 0)] = buf[ISO7816.OFFSET_CLA];
        apduLog[(short) (slot + 1)] = buf[ISO7816.OFFSET_INS];
        apduLog[(short) (slot + 2)] = buf[ISO7816.OFFSET_P1];
        apduLog[(short) (slot + 3)] = buf[ISO7816.OFFSET_P2];
        apduLog[(short) (slot + 4)] = buf[ISO7816.OFFSET_LC];
        Util.arrayFillNonAtomic(apduLog, (short) (slot + APDU_LOG_DATA_OFF), APDU_LOG_DATA_LEN, (byte) 0);
        apduLog[(short) (slot + APDU_LOG_SW_OFF)] = APDU_LOG_SW_PENDING;
        apduLog[(short) (slot + APDU_LOG_SW_OFF + 1)] = APDU_LOG_SW_PENDING;
        short next = (short) (slot + APDU_LOG_ENTRY);
        if (next >= APDU_LOG_SIZE) next = 0;
        apduLogHead = next;
        return slot;
    }

    /** Keeps the first data bytes of the current command, a FID or an
     *  AID prefix, in its trace entry. Never called for PIN commands. */
    private void logApduData(byte[] buf, short lc) {
        if (currentLogSlot < 0 || lc <= 0) return;
        short n = lc < APDU_LOG_DATA_LEN ? lc : APDU_LOG_DATA_LEN;
        Util.arrayCopyNonAtomic(buf, ISO7816.OFFSET_CDATA, apduLog,
                                (short) (currentLogSlot + APDU_LOG_DATA_OFF), n);
    }

    private void logApduSw(short slot, short sw) {
        if (slot < 0) return;
        apduLog[(short) (slot + APDU_LOG_SW_OFF)] = (byte) ((sw >> 8) & 0xFF);
        apduLog[(short) (slot + APDU_LOG_SW_OFF + 1)] = (byte) (sw & 0xFF);
    }

    /** P1P2 = offset into the trace. Response: the head offset (2 bytes),
     *  then up to 254 trace bytes from the offset. The entry at the head
     *  is the oldest. */
    private void doDebugDumpApduLog(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        short off = Util.getShort(buf, ISO7816.OFFSET_P1);
        if (off < 0 || off >= APDU_LOG_SIZE) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        short n = (short) (APDU_LOG_SIZE - off);
        if (n > APDU_LOG_DUMP_MAX) n = APDU_LOG_DUMP_MAX;
        Util.setShort(scratch, (short) 0, apduLogHead);
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) (n + 2));
        apdu.sendBytesLong(scratch, (short) 0, (short) 2);
        apdu.sendBytesLong(apduLog, off, n);
    }

    private void doDebugClearApduLog(APDU apdu) {
        Util.arrayFillNonAtomic(apduLog, (short) 0, APDU_LOG_SIZE, (byte) 0);
        apduLogHead = 0;
    }

    // SELECT (TS 102 221 §11.1.1)

    private void doSelect(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte p2 = buf[ISO7816.OFFSET_P2];
        short lc = apdu.setIncomingAndReceive();
        logApduData(apdu.getBuffer(), lc);

        switch (p1) {
            case SEL_BY_FID: {
                // An empty data field selects the MF (TS 102 221 §11.1.1.2).
                if (lc == 0) { selectDf(apdu, channel, DF_MF, p2); return; }
                if (lc != 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                selectFid(apdu, channel, Util.getShort(buf, ISO7816.OFFSET_CDATA), p2);
                return;
            }
            case SEL_CHILD_DF:
                // No DFs below the MF or the ADF are present.
                ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
                return;
            case SEL_PARENT_DF: {
                // From an EF or the ADF the parent is the ADF resp. the MF.
                if (fs.current(channel) == null && dfOfChannel[channel] == DF_MF) {
                    ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
                }
                if (fs.current(channel) != null && dfOfChannel[channel] == DF_ADF) {
                    selectDf(apdu, channel, DF_ADF, p2);
                } else {
                    selectDf(apdu, channel, DF_MF, p2);
                }
                return;
            }
            case SEL_BY_DF_NAME: {
                // The JCRE resolves SELECT by AID itself. This path is
                // reached for a partial DF name the JCRE could not match
                // (TS 102 221 §8.4.2, C.S0065-B §7.1.1.1).
                if (lc < AID_MIN_PARTIAL || lc > (short) CSIM_AID.length
                        || Util.arrayCompare(buf, ISO7816.OFFSET_CDATA,
                                             CSIM_AID, (short) 0, lc) != 0) {
                    ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
                }
                selectDf(apdu, channel, DF_ADF, p2);
                return;
            }
            case SEL_PATH_FROM_MF:
            case SEL_PATH_FROM_DF: {
                if (lc < 2 || (lc & 1) != 0) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                selectPath(apdu, channel, lc, p1 == SEL_PATH_FROM_MF, p2);
                return;
            }
            default:
                ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    private void selectFid(APDU apdu, byte channel, short fid, byte p2) {
        if (fid == Const.FID_MF) {
            selectDf(apdu, channel, DF_MF, p2);
            return;
        }
        if (fid == Const.FID_ADF) {
            selectDf(apdu, channel, DF_ADF, p2);
            return;
        }
        Ef ef = fs.find(fid);
        if (ef == null) ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        // An EF is selectable by FID from its own DF only (TS 102 221 §8.4.1).
        boolean adfEf = FileSystem.isAdfEf(fid);
        if (adfEf != (dfOfChannel[channel] == DF_ADF)) {
            ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        }
        selectEf(apdu, channel, ef, p2);
    }

    private void selectDf(APDU apdu, byte channel, byte df, byte p2) {
        dfOfChannel[channel] = df;
        fs.select(channel, null);
        selectedRecord[channel] = 0;
        if (df == DF_ADF) sendFcpForAdf(apdu, p2);
        else               sendFcpForMf(apdu, p2);
    }

    private void selectEf(APDU apdu, byte channel, Ef ef, byte p2) {
        fs.select(channel, ef);
        selectedRecord[channel] = 0;
        sendFcpForEf(apdu, ef, p2);
        if (ef.deactivated) {
            ISOException.throwIt(Const.SW_FILE_DEACTIVATED);
        }
    }

    /** Path selection. Each element is a FID. '3F00' and '7FFF' switch
     *  the DF context, an EF must be the last element. */
    private void selectPath(APDU apdu, byte channel, short lc, boolean fromMf, byte p2) {
        byte[] buf = apdu.getBuffer();
        short lastOff = (short) (ISO7816.OFFSET_CDATA + lc - 2);
        short fid = Util.getShort(buf, lastOff);
        byte df = pathDf(buf, lc, fromMf ? DF_MF : dfOfChannel[channel]);
        if (fid == Const.FID_MF) { selectDf(apdu, channel, DF_MF, p2); return; }
        if (fid == Const.FID_ADF) { selectDf(apdu, channel, DF_ADF, p2); return; }
        Ef ef = pathEf(fid, df);
        dfOfChannel[channel] = df;
        selectEf(apdu, channel, ef, p2);
    }

    /** DF context after the path elements before the last one. Only the
     *  MF and the ADF exist as DFs. */
    private byte pathDf(byte[] buf, short lc, byte df) {
        short off = ISO7816.OFFSET_CDATA;
        short lastOff = (short) (ISO7816.OFFSET_CDATA + lc - 2);
        while (off < lastOff) {
            short fid = Util.getShort(buf, off);
            off += 2;
            if (fid == Const.FID_MF) df = DF_MF;
            else if (fid == Const.FID_ADF) df = DF_ADF;
            else ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        }
        return df;
    }

    private Ef pathEf(short fid, byte df) {
        Ef ef = fs.find(fid);
        if (ef == null || FileSystem.isAdfEf(fid) != (df == DF_ADF)) {
            ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        }
        return ef;
    }

    // FCP builders (TS 102 221 §11.1.1.3)

    private void putTlv1(byte tag, byte value) {
        scratch[currentOff++] = tag;
        scratch[currentOff++] = 1;
        scratch[currentOff++] = value;
    }

    private void putTlv2(byte tag, short value) {
        scratch[currentOff++] = tag;
        scratch[currentOff++] = 2;
        Util.setShort(scratch, currentOff, value);
        currentOff += 2;
    }

    /** Security attributes of an EF: READ, UPDATE, INCREASE (cyclic EFs
     *  only, AM_DO tag 84 with INS 32, TS 102 221 §11.1.8), DEACTIVATE
     *  and ACTIVATE, each as one expanded-format rule. */
    private void putEfSecurity(Ef ef) {
        scratch[currentOff++] = TAG_SEC_EXPANDED;
        short lenPos = currentOff++;
        putRule(AM_DO_BYTE, AM_READ, ef.readAc);
        putRule(AM_DO_BYTE, AM_UPDATE, ef.updateAc);
        if (ef.structure == Const.FS_CYCLIC) {
            putRule(AM_DO_INS, Const.INS_INCREASE, FileSystem.increaseAc(ef.fid, ef.updateAc));
        }
        putRule(AM_DO_BYTE, AM_DEACTIVATE, FileSystem.deactivateAc(ef.fid));
        putRule(AM_DO_BYTE, AM_ACTIVATE, FileSystem.activateAc(ef.fid));
        scratch[lenPos] = (byte) (currentOff - lenPos - 1);
    }

    /** One expanded-format rule: AM_DO (tag 80 AM byte, or tag 84 INS)
     *  followed by its SC_DO (ISO/IEC 7816-4 §5.4.3.3). */
    private void putRule(byte amTag, byte amValue, byte acCode) {
        scratch[currentOff++] = amTag;
        scratch[currentOff++] = 1;
        scratch[currentOff++] = amValue;
        switch (acCode) {
            case Const.AC_ALWAYS:
                scratch[currentOff++] = SC_DO_ALWAYS;
                scratch[currentOff++] = 0;
                break;
            case Const.AC_NEVER:
                scratch[currentOff++] = SC_DO_NEVER;
                scratch[currentOff++] = 0;
                break;
            default:
                scratch[currentOff++] = SC_DO_AUTH;
                scratch[currentOff++] = 6;
                scratch[currentOff++] = TAG_KEY_REF;
                scratch[currentOff++] = 1;
                scratch[currentOff++] = (acCode == Const.AC_PIN1) ? P2_PIN1
                        : (acCode == Const.AC_PIN2) ? P2_PIN2 : P2_ADM1;
                scratch[currentOff++] = TAG_USAGE_QUALIFIER;
                scratch[currentOff++] = 1;
                scratch[currentOff++] = USAGE_PIN_VERIFICATION;
        }
    }

    private static final byte PS_TEMPLATE_LEN = 12;   // PS_DO (3) + three key references (3 each)
    private static final byte PS_DO_PIN1 = (byte) 0x80;
    private static final byte PS_DO_PIN2 = 0x40;
    private static final byte PS_DO_ADM1 = 0x20;

    /** PIN status template DO (TS 102 221 §11.1.1.4.10): PS_DO flags each
     *  listed key reference enabled (1) or disabled (0), b8 first. */
    private void putPinStatusTemplate() {
        scratch[currentOff++] = TAG_PIN_STATUS_TEMPLATE;
        scratch[currentOff++] = PS_TEMPLATE_LEN;
        scratch[currentOff++] = TAG_PS_DO;
        scratch[currentOff++] = 1;
        scratch[currentOff++] = (byte) ((ac.isPin1Enabled() ? PS_DO_PIN1 : 0) | PS_DO_PIN2 | PS_DO_ADM1);
        putTlv1(TAG_KEY_REF, P2_PIN1);
        putTlv1(TAG_KEY_REF, P2_PIN2);
        putTlv1(TAG_KEY_REF, P2_ADM1);
    }

    private void sendFcp(APDU apdu) {
        scratch[1] = (byte) (currentOff - 2);
        apdu.setOutgoing();
        apdu.setOutgoingLength(currentOff);
        apdu.sendBytesLong(scratch, (short) 0, currentOff);
    }

    // File descriptor DO lengths (TS 102 221 §11.1.1.4.3): descriptor byte
    // and data coding, plus a 2-byte record length and a record count for
    // record EFs.
    private static final byte FD_LEN_TRANSPARENT = 2;
    private static final byte FD_LEN_RECORD = 5;

    private void sendFcpForEf(APDU apdu, Ef ef, byte p2) {
        if ((p2 & SEL_P2_DATA_MASK) == SEL_P2_NO_DATA) return;
        currentOff = 0;
        scratch[currentOff++] = TAG_FCP;
        currentOff++;
        scratch[currentOff++] = TAG_FILE_DESCRIPTOR;
        if (ef.structure == Const.FS_TRANSPARENT) {
            scratch[currentOff++] = FD_LEN_TRANSPARENT;
            scratch[currentOff++] = (byte) (FD_EF_SHAREABLE | Const.FS_TRANSPARENT);
            scratch[currentOff++] = DATA_CODING;
        } else {
            scratch[currentOff++] = FD_LEN_RECORD;
            scratch[currentOff++] = (byte) (FD_EF_SHAREABLE | ef.structure);
            scratch[currentOff++] = DATA_CODING;
            scratch[currentOff++] = 0x00;
            scratch[currentOff++] = ef.recordLength;
            scratch[currentOff++] = (byte) ef.recordCount();
        }
        putTlv2(TAG_FILE_ID, ef.fid);
        putTlv1(TAG_LIFE_CYCLE, ef.deactivated ? LCS_DEACTIVATED : LCS_ACTIVATED);
        putEfSecurity(ef);
        putTlv2(TAG_FILE_SIZE, ef.capacity());
        if (ef.sfi != 0) {
            putTlv1(TAG_SFI, (byte) ((ef.sfi & SFI_MASK) << SFI_SHIFT));
        }
        sendFcp(apdu);
    }

    private void sendFcpForDf(APDU apdu, boolean adf, byte p2) {
        if ((p2 & SEL_P2_DATA_MASK) == SEL_P2_NO_DATA) return;
        currentOff = 0;
        scratch[currentOff++] = TAG_FCP;
        currentOff++;
        scratch[currentOff++] = TAG_FILE_DESCRIPTOR;
        scratch[currentOff++] = 2;
        scratch[currentOff++] = FD_DF;
        scratch[currentOff++] = DATA_CODING;
        putTlv2(TAG_FILE_ID, adf ? Const.FID_ADF : Const.FID_MF);
        if (adf) {
            scratch[currentOff++] = TAG_DF_NAME;
            scratch[currentOff++] = (byte) CSIM_AID.length;
            Util.arrayCopyNonAtomic(CSIM_AID, (short) 0, scratch, currentOff,
                                    (short) CSIM_AID.length);
            currentOff += (short) CSIM_AID.length;
        }
        scratch[currentOff++] = TAG_PROPRIETARY;
        scratch[currentOff++] = 3;
        putTlv1(TAG_UICC_CHARACTERISTICS, UICC_CHARACTERISTICS);
        putTlv1(TAG_LIFE_CYCLE, LCS_ACTIVATED);
        scratch[currentOff++] = TAG_SEC_EXPANDED;
        scratch[currentOff++] = 5;
        putRule(AM_DO_BYTE, AM_DF_DELETE, Const.AC_NEVER);
        putPinStatusTemplate();
        sendFcp(apdu);
    }

    private void sendFcpForAdf(APDU apdu, byte p2) {
        sendFcpForDf(apdu, true, p2);
    }

    private void sendFcpForMf(APDU apdu, byte p2) {
        sendFcpForDf(apdu, false, p2);
    }

    // STATUS (TS 102 221 §11.1.2)

    private void doStatus(APDU apdu, byte channel) {
        byte p2 = apdu.getBuffer()[ISO7816.OFFSET_P2];
        boolean adf = dfOfChannel[channel] == DF_ADF;
        if (p2 == STATUS_P2_NO_DATA) return;
        if (p2 == STATUS_P2_DF_NAME) {
            if (!adf) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            short n = (short) CSIM_AID.length;
            scratch[0] = TAG_DF_NAME;
            scratch[1] = (byte) n;
            Util.arrayCopyNonAtomic(CSIM_AID, (short) 0, scratch, (short) 2, n);
            short total = (short) (n + 2);
            apdu.setOutgoing();
            apdu.setOutgoingLength(total);
            apdu.sendBytesLong(scratch, (short) 0, total);
            return;
        }
        sendFcpForDf(apdu, adf, (byte) 0);
    }

    // READ / UPDATE BINARY (TS 102 221 §11.1.3 / §11.1.4)

    /** P1 b8=1, b7 b6=00: P1 b5..b1 is an SFI and P2 the offset. Else
     *  P1 b7..b1 || P2 is the offset into the current EF. */
    private Ef resolveBinaryAddr(APDU apdu, byte channel, short[] outOffset) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte p2 = buf[ISO7816.OFFSET_P2];
        Ef ef;
        if ((p1 & P1_SFI_FLAG) != 0) {
            if ((p1 & P1_SFI_RFU) != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            ef = fs.findBySfi((byte) (p1 & SFI_MASK), dfOfChannel[channel] == DF_ADF);
            if (ef == null) ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
            fs.select(channel, ef);
            selectedRecord[channel] = 0;
            outOffset[0] = (short) (p2 & 0xFF);
        } else {
            outOffset[0] = (short) (((p1 & 0x7F) << 8) | (p2 & 0xFF));
            ef = currentEf(channel);
        }
        if (ef.structure != Const.FS_TRANSPARENT) {
            ISOException.throwIt(Const.SW_INCOMPATIBLE_FILE);
        }
        if (ef.deactivated) ISOException.throwIt(Const.SW_NO_EF_SELECTED);
        return ef;
    }

    private void doReadBinary(APDU apdu, byte channel) {
        Ef ef = resolveBinaryAddr(apdu, channel, tmpOffset);
        ac.check(ef.readAc, channel);
        short off = tmpOffset[0];
        if (off >= ef.capacity()) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        short le = apdu.setOutgoing();
        if (le == 0 || le > READ_BINARY_MAX) le = READ_BINARY_MAX;
        short avail = (short) (ef.capacity() - off);
        if (le > avail) le = avail;
        apdu.setOutgoingLength(le);
        byte[] buf = apdu.getBuffer();
        Util.arrayCopyNonAtomic(ef.pool, (short) (ef.poolOffset + off),
                                buf, (short) 0, le);
        apdu.sendBytes((short) 0, le);
    }

    private void doUpdateBinary(APDU apdu, byte channel) {
        Ef ef = resolveBinaryAddr(apdu, channel, tmpOffset);
        ac.check(ef.updateAc, channel);
        short off = tmpOffset[0];
        byte[] buf = apdu.getBuffer();
        short lc = apdu.setIncomingAndReceive();
        if (off >= ef.capacity()) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        if ((short) (off + lc) > ef.capacity()) {
            ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        }
        write(buf, ISO7816.OFFSET_CDATA, ef.pool, (short) (ef.poolOffset + off), lc, channel);
        if (ef.fid == Const.FID_EF_SPC) spcWritten();
    }

    // READ / UPDATE RECORD (TS 102 221 §11.1.5 / §11.1.6)

    /** P2 b8..b4 = SFI (0 = current EF), b3..b1 = mode. */
    private Ef resolveRecordEf(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte sfi = (byte) ((buf[ISO7816.OFFSET_P2] >> SFI_SHIFT) & SFI_MASK);
        Ef ef;
        if (sfi != 0) {
            ef = fs.findBySfi(sfi, dfOfChannel[channel] == DF_ADF);
            if (ef == null) ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
            fs.select(channel, ef);
            selectedRecord[channel] = 0;
        } else {
            ef = currentEf(channel);
        }
        if (ef.structure == Const.FS_TRANSPARENT) {
            ISOException.throwIt(Const.SW_INCOMPATIBLE_FILE);
        }
        if (ef.deactivated) ISOException.throwIt(Const.SW_NO_EF_SELECTED);
        return ef;
    }

    /**
     * Resolve P1/P2 record addressing to a 1-based record number. In a
     * cyclic EF record 1 is the most recently written one, NEXT wraps
     * from the last record to record 1 and PREVIOUS from record 1 to
     * the last (TS 102 221 §8.2.2.4).
     */
    private short pickRecord(Ef ef, byte channel, byte p1, byte p2) {
        short n = ef.recordCount();
        short cur = (short) (selectedRecord[channel] & 0xFF);
        boolean cyclic = ef.structure == Const.FS_CYCLIC;
        switch ((byte) (p2 & REC_MODE_MASK)) {
            case REC_ABSOLUTE: {
                short rec = (short) (p1 & 0xFF);
                if (rec == 0) {
                    if (cur == 0) ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
                    return cur;
                }
                if (rec > n) ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
                return rec;
            }
            case REC_NEXT: {
                if (p1 != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                if (cur == 0) return 1;
                if (cur >= n) {
                    if (cyclic) return 1;
                    ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
                }
                return (short) (cur + 1);
            }
            case REC_PREVIOUS: {
                if (p1 != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                if (cur == 0) return n;
                if (cur <= 1) {
                    if (cyclic) return n;
                    ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
                }
                return (short) (cur - 1);
            }
            default:
                ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                return 0;
        }
    }

    private void doReadRecord(APDU apdu, byte channel) {
        Ef ef = resolveRecordEf(apdu, channel);
        ac.check(ef.readAc, channel);
        byte[] buf = apdu.getBuffer();
        // The record data overwrites the APDU header below, so read P2 first.
        byte mode = (byte) (buf[ISO7816.OFFSET_P2] & REC_MODE_MASK);
        short rec = pickRecord(ef, channel, buf[ISO7816.OFFSET_P1], buf[ISO7816.OFFSET_P2]);
        short recLen = (short) (ef.recordLength & 0xFF);
        short le = apdu.setOutgoing();
        if (le == 0 || le > recLen) le = recLen;
        apdu.setOutgoingLength(le);
        Util.arrayCopyNonAtomic(ef.pool, ef.recordOffset(rec), buf, (short) 0, le);
        apdu.sendBytes((short) 0, le);
        // Only NEXT and PREVIOUS move the record pointer (TS 102 221 §11.1.5).
        if (mode != REC_ABSOLUTE) selectedRecord[channel] = (byte) rec;
    }

    private void doUpdateRecord(APDU apdu, byte channel) {
        Ef ef = resolveRecordEf(apdu, channel);
        ac.check(ef.updateAc, channel);
        byte[] buf = apdu.getBuffer();
        byte mode = (byte) (buf[ISO7816.OFFSET_P2] & REC_MODE_MASK);
        boolean cyclic = ef.structure == Const.FS_CYCLIC;
        // Only PREVIOUS is allowed for cyclic files (TS 102 221 §11.1.6).
        if (cyclic && mode != REC_PREVIOUS) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        short rec = pickRecord(ef, channel, buf[ISO7816.OFFSET_P1], buf[ISO7816.OFFSET_P2]);
        short lc = apdu.setIncomingAndReceive();
        short recLen = (short) (ef.recordLength & 0xFF);
        if (lc != recLen) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        boolean txn = cyclic && begin(channel);
        if (cyclic) {
            // The oldest record is overwritten and becomes record 1.
            rotateCyclic(ef, channel);
            rec = 1;
        }
        write(buf, ISO7816.OFFSET_CDATA, ef.pool, ef.recordOffset(rec), lc, channel);
        commit(txn);
        if (mode != REC_ABSOLUTE) selectedRecord[channel] = (byte) rec;
    }

    /** Writes on a channel without ADM1 come from the handset or the
     *  network and must survive a power loss mid-write. ADM1
     *  personalization writes skip the journal, since an interrupted
     *  csim write is run again. */
    private boolean tearSafe(byte channel) {
        return !ac.isAdmVerified(channel);
    }

    private void write(byte[] src, short srcOff, byte[] dst, short dstOff, short len, byte channel) {
        if (tearSafe(channel)) {
            Util.arrayCopy(src, srcOff, dst, dstOff, len);
        } else {
            Util.arrayCopyNonAtomic(src, srcOff, dst, dstOff, len);
        }
    }

    /** Opens a transaction for a tear-safe write that changes more than
     *  one region. The JCRE aborts it when the command throws. */
    private boolean begin(byte channel) {
        if (!tearSafe(channel) || JCSystem.getTransactionDepth() != 0) return false;
        JCSystem.beginTransaction();
        return true;
    }

    private static void commit(boolean started) {
        if (started) JCSystem.commitTransaction();
    }

    /** Shift every record of a cyclic EF down by one. The former last
     *  record is dropped and record 1 is free for the new value. */
    private void rotateCyclic(Ef ef, byte channel) {
        short n = ef.recordCount();
        short recLen = (short) (ef.recordLength & 0xFF);
        for (short r = (short) (n - 1); r >= 1; r--) {
            write(ef.pool, ef.recordOffset(r), ef.pool, ef.recordOffset((short) (r + 1)), recLen, channel);
        }
    }

    // SEARCH RECORD (TS 102 221 §11.1.7), simple search

    /** Simple search (TS 102 221 §11.1.7): P2 b3..b1 = 100 forward or
     *  101 backward from the record in P1 (0 = current record), data =
     *  pattern matched at offset 0 of each record. Enhanced and
     *  proprietary search are not supported. Returns the matching record
     *  numbers, at most Le of them, and sets the record pointer to the
     *  first match. */
    private void doSearchRecord(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        Ef ef = resolveRecordEf(apdu, channel);
        ac.check(ef.readAc, channel);
        byte mode = (byte) (buf[ISO7816.OFFSET_P2] & REC_MODE_MASK);
        if (mode != SEARCH_FORWARD && mode != SEARCH_BACKWARD) {
            ISOException.throwIt(Const.SW_FUNCTION_NOT_SUPPORTED);
        }
        boolean forward = mode == SEARCH_FORWARD;
        short lc = apdu.setIncomingAndReceive();
        short recLen = (short) (ef.recordLength & 0xFF);
        if (lc == 0 || lc > recLen) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        short n = ef.recordCount();
        short start = (short) (buf[ISO7816.OFFSET_P1] & 0xFF);
        if (start == 0) {
            start = (short) (selectedRecord[channel] & 0xFF);
            if (start == 0) start = forward ? (short) 1 : n;
        }
        if (start > n) ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
        short found = 0;
        for (short r = start; r >= 1 && r <= n; r = (short) (forward ? r + 1 : r - 1)) {
            if (Util.arrayCompare(ef.pool, ef.recordOffset(r),
                                  buf, ISO7816.OFFSET_CDATA, lc) == 0) {
                scratch[found++] = (byte) r;
            }
        }
        if (found == 0) ISOException.throwIt(Const.SW_RECORD_NOT_FOUND);
        selectedRecord[channel] = scratch[0];
        short le = apdu.setOutgoing();
        if (le != 0 && le < found) found = le;
        apdu.setOutgoingLength(found);
        apdu.sendBytesLong(scratch, (short) 0, found);
    }

    // INCREASE (TS 102 221 §11.1.8)

    /**
     * Adds the big-endian value in the data field (1 to 127 bytes) to
     * record 1 of a cyclic EF and stores the result as the new record 1
     * (the oldest record is dropped). Response: the new record followed
     * by the value added. 9850 when the result does not fit the record.
     * P1 = 00 for the current EF, or b8 set with the SFI in b5..b1.
     */
    private void doIncrease(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        if (buf[ISO7816.OFFSET_P2] != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        Ef ef;
        if (p1 == 0) {
            ef = currentEf(channel);
        } else if ((p1 & P1_SFI_PREFIX_MASK) == P1_SFI_FLAG) {
            ef = fs.findBySfi((byte) (p1 & SFI_MASK), dfOfChannel[channel] == DF_ADF);
            if (ef == null) ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
            fs.select(channel, ef);
            selectedRecord[channel] = 0;
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            return;
        }
        if (ef.structure != Const.FS_CYCLIC) {
            ISOException.throwIt(Const.SW_INCOMPATIBLE_FILE);
        }
        if (ef.deactivated) ISOException.throwIt(Const.SW_NO_EF_SELECTED);
        ac.check(FileSystem.increaseAc(ef.fid, ef.updateAc), channel);
        short lc = apdu.setIncomingAndReceive();
        if (lc < 1 || lc > INCREASE_MAX_DATA_LEN) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        short recLen = (short) (ef.recordLength & 0xFF);
        if (recLen > INCREASE_MAX_DATA_LEN) ISOException.throwIt(Const.SW_INCOMPATIBLE_FILE);
        // scratch[0..recLen) = new record value, then the added value.
        Util.arrayCopyNonAtomic(ef.pool, ef.recordOffset((short) 1), scratch, (short) 0, recLen);
        short carry = 0;
        for (short i = (short) (recLen - 1); i >= 0; i--) {
            short addIdx = (short) (i - (short) (recLen - lc));
            short add = 0;
            if (addIdx >= 0 && addIdx < lc) {
                add = (short) (buf[(short) (ISO7816.OFFSET_CDATA + addIdx)] & 0xFF);
            }
            short sum = (short) ((scratch[i] & 0xFF) + add + carry);
            scratch[i] = (byte) (sum & 0xFF);
            carry = (short) ((sum >> 8) & 0x01);
        }
        if (carry != 0) ISOException.throwIt(Const.SW_INCREASE_MAX_VALUE);
        // Bytes of the added value above the record width must be zero.
        for (short i = 0; i < (short) (lc - recLen); i++) {
            if (buf[(short) (ISO7816.OFFSET_CDATA + i)] != 0) {
                ISOException.throwIt(Const.SW_INCREASE_MAX_VALUE);
            }
        }
        boolean txn = begin(channel);
        rotateCyclic(ef, channel);
        write(scratch, (short) 0, ef.pool, ef.recordOffset((short) 1), recLen, channel);
        commit(txn);
        selectedRecord[channel] = 1;
        Util.arrayCopyNonAtomic(buf, ISO7816.OFFSET_CDATA, scratch, recLen, lc);
        short total = (short) (recLen + lc);
        apdu.setOutgoing();
        apdu.setOutgoingLength(total);
        apdu.sendBytesLong(scratch, (short) 0, total);
    }

    // DEACTIVATE / ACTIVATE FILE (TS 102 221 §11.1.14 / §11.1.15)

    /** P1 = 00 with no data: the current EF. P1 = 00 with a FID, or
     *  P1 = 08/09 with a path: that EF, which then becomes the current EF
     *  (TS 102 221 §11.1.14). */
    private void doActivateFile(APDU apdu, byte channel, boolean activate) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        if (buf[ISO7816.OFFSET_P2] != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        short lc = apdu.setIncomingAndReceive();
        Ef ef;
        byte df = dfOfChannel[channel];
        if (p1 == SEL_BY_FID && lc == 0) {
            ef = currentEf(channel);
        } else if (p1 == SEL_BY_FID && lc == 2) {
            ef = pathEf(Util.getShort(buf, ISO7816.OFFSET_CDATA), df);
        } else if ((p1 == SEL_PATH_FROM_MF || p1 == SEL_PATH_FROM_DF) && lc >= 2 && (lc & 1) == 0) {
            df = pathDf(buf, lc, p1 == SEL_PATH_FROM_MF ? DF_MF : df);
            ef = pathEf(Util.getShort(buf, (short) (ISO7816.OFFSET_CDATA + lc - 2)), df);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            return;
        }
        byte acCode = activate ? FileSystem.activateAc(ef.fid) : FileSystem.deactivateAc(ef.fid);
        ac.check(acCode, channel);
        if (ef.deactivated != activate) {
            ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        }
        fs.setDeactivated(ef, !activate);
        dfOfChannel[channel] = df;
        fs.select(channel, ef);
        selectedRecord[channel] = 0;
    }

    // PIN commands (TS 102 221 §11.1.9 .. §11.1.13)

    private OwnerPIN pinByRef(byte p2) {
        switch (p2) {
            case P2_PIN1: return ac.pin1;
            case P2_PIN2: return ac.pin2;
            case P2_ADM1: return ac.adm1;
            default:
                ISOException.throwIt(Const.SW_REF_DATA_NOT_FOUND);
                return null;
        }
    }

    private OwnerPIN pukFor(byte p2) {
        if (p2 == P2_PIN1) return ac.puk1;
        if (p2 == P2_PIN2) return ac.puk2;
        ISOException.throwIt(Const.SW_REF_DATA_NOT_FOUND);
        return null;
    }

    private void markVerified(byte ref, byte channel) {
        if (ref == P2_PIN1) ac.setPin1Verified(channel, true);
        else if (ref == P2_PIN2) ac.setPin2Verified(channel, true);
        else if (ref == P2_ADM1) ac.setAdmVerified(channel, true);
    }

    private static short triesSw(OwnerPIN p) {
        return (short) (Const.SW_VERIFY_FAILED_BASE | (p.getTriesRemaining() & 0x0F));
    }

    /** VERIFY PIN. An empty data field queries the remaining attempts
     *  (63CX), whether or not the PIN is disabled. Verification of a
     *  disabled PIN is a condition of use not satisfied (TS 102 221
     *  §11.1.9.1.1). Its files are accessible without it. */
    private void doVerifyPin(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte ref = buf[ISO7816.OFFSET_P2];
        OwnerPIN p = pinByRef(ref);
        short lc = apdu.setIncomingAndReceive();
        if (lc == PIN_STATUS_QUERY_LEN) {
            ISOException.throwIt(triesSw(p));
        }
        if (ref == P2_PIN1 && !ac.isPin1Enabled()) {
            ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        }
        if (lc != Const.PIN_MAX_SIZE) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (p.getTriesRemaining() == 0) {
            ISOException.throwIt(Const.SW_AUTH_METHOD_BLOCKED);
        }
        if (!p.check(buf, ISO7816.OFFSET_CDATA, (byte) lc)) {
            ISOException.throwIt(triesSw(p));
        }
        markVerified(ref, channel);
    }

    private static final byte SET_PIN_P1_PIN = 0x00;
    private static final byte SET_PIN_P1_PUK = 0x01;

    /** SET PIN: P1 = 00 sets PIN1 or PIN2, P1 = 01 sets PUK1 or PUK2, P2 =
     *  01 or 81, data = the new 8-byte value. Resets its retry counter,
     *  which also unblocks it. ADM1 required. */
    private void doSetPin(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        OwnerPIN target;
        switch (buf[ISO7816.OFFSET_P1]) {
            case SET_PIN_P1_PIN:
                if (p2 != P2_PIN1 && p2 != P2_PIN2) ISOException.throwIt(Const.SW_REF_DATA_NOT_FOUND);
                target = pinByRef(p2);
                break;
            case SET_PIN_P1_PUK:
                target = pukFor(p2);
                break;
            default:
                ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                return;
        }
        short lc = apdu.setIncomingAndReceive();
        if (lc != Const.PIN_MAX_SIZE) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        target.update(buf, ISO7816.OFFSET_CDATA, Const.PIN_MAX_SIZE);
    }

    /** CHANGE PIN: data = old PIN (8) || new PIN (8). Only for an
     *  enabled, unblocked PIN. */
    private void doChangePin(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        byte ref = buf[ISO7816.OFFSET_P2];
        OwnerPIN p = pinByRef(ref);
        short lc = apdu.setIncomingAndReceive();
        if (lc != (short) (2 * Const.PIN_MAX_SIZE)) {
            ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        }
        if (ref == P2_PIN1 && !ac.isPin1Enabled()) {
            ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        }
        if (p.getTriesRemaining() == 0) {
            ISOException.throwIt(Const.SW_AUTH_METHOD_BLOCKED);
        }
        byte half = Const.PIN_MAX_SIZE;
        if (!p.check(buf, ISO7816.OFFSET_CDATA, half)) {
            ISOException.throwIt(triesSw(p));
        }
        p.update(buf, (short) (ISO7816.OFFSET_CDATA + half), half);
    }

    /** DISABLE PIN (INS 26) / ENABLE PIN (INS 28). Only PIN1 can be
     *  disabled. Data = PIN value. Changing to the current state is
     *  refused with 6985. */
    private void doEnablePin(APDU apdu, boolean enable) {
        byte[] buf = apdu.getBuffer();
        byte ref = buf[ISO7816.OFFSET_P2];
        if (ref != P2_PIN1) {
            ISOException.throwIt(Const.SW_REF_DATA_NOT_FOUND);
        }
        if (buf[ISO7816.OFFSET_P1] != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        short lc = apdu.setIncomingAndReceive();
        if (lc != Const.PIN_MAX_SIZE) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (ac.isPin1Enabled() == enable) {
            ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        }
        if (ac.pin1.getTriesRemaining() == 0) {
            ISOException.throwIt(Const.SW_AUTH_METHOD_BLOCKED);
        }
        if (!ac.pin1.check(buf, ISO7816.OFFSET_CDATA, (byte) lc)) {
            ISOException.throwIt(triesSw(ac.pin1));
        }
        ac.setPin1Enabled(enable);
    }

    private static final byte UNBLOCK_P1_NEW_PIN = 0x00;
    private static final byte UNBLOCK_P1_KEEP_PIN = 0x01;

    /**
     * UNBLOCK PIN. P1 = 00: data = PUK (8) || new PIN (8). P1 = 01: data
     * = PUK only, the PIN keeps its value. An empty data field queries
     * the remaining PUK attempts (63CX). P2 = PIN1 or PIN2 reference.
     */
    private void doUnblockPin(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte ref = buf[ISO7816.OFFSET_P2];
        OwnerPIN targetPin;
        OwnerPIN targetPuk;
        if (ref == P2_PIN1) {
            targetPin = ac.pin1; targetPuk = ac.puk1;
        } else if (ref == P2_PIN2) {
            targetPin = ac.pin2; targetPuk = ac.puk2;
        } else {
            ISOException.throwIt(Const.SW_REF_DATA_NOT_FOUND);
            return;
        }
        short lc = apdu.setIncomingAndReceive();
        if (lc == PIN_STATUS_QUERY_LEN) {
            ISOException.throwIt(triesSw(targetPuk));
        }
        if (targetPuk.getTriesRemaining() == 0) {
            ISOException.throwIt(Const.SW_AUTH_METHOD_BLOCKED);
        }
        byte pukLen = Const.PUK_MAX_SIZE;
        if (p1 == UNBLOCK_P1_NEW_PIN) {
            if (lc != (short) (pukLen + Const.PIN_MAX_SIZE)) {
                ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            }
            if (!targetPuk.check(buf, ISO7816.OFFSET_CDATA, pukLen)) {
                ISOException.throwIt(triesSw(targetPuk));
            }
            targetPin.update(buf, (short) (ISO7816.OFFSET_CDATA + pukLen),
                    Const.PIN_MAX_SIZE);
        } else if (p1 == UNBLOCK_P1_KEEP_PIN) {
            if (lc != pukLen) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            if (!targetPuk.check(buf, ISO7816.OFFSET_CDATA, pukLen)) {
                ISOException.throwIt(triesSw(targetPuk));
            }
            targetPin.resetAndUnblock();
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            return;
        }
        // After unblocking the PIN is enabled and its access condition is
        // satisfied (TS 102 221 §11.1.13.1.1).
        if (ref == P2_PIN1) ac.setPin1Enabled(true);
        markVerified(ref, channel);
    }

    // MANAGE CHANNEL (TS 102 221 §11.1.17)

    private static final byte MANAGE_CHANNEL_P1_OPEN = 0x00;
    private static final byte MANAGE_CHANNEL_P1_CLOSE = (byte) 0x80;
    private static final byte MANAGE_CHANNEL_P2_MASK = 0x03;

    /** The JCRE normally services MANAGE CHANNEL itself. This handler
     *  covers a platform that forwards it to the applet. */
    private void doManageChannel(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (p1 == MANAGE_CHANNEL_P1_OPEN) {
            for (byte c = 1; c < FileSystem.MAX_CHANNELS; c++) {
                if (fs.current(c) == null && dfOfChannel[c] == DF_MF) {
                    apdu.setOutgoing();
                    apdu.setOutgoingLength((short) 1);
                    buf[0] = c;
                    apdu.sendBytes((short) 0, (short) 1);
                    return;
                }
            }
            ISOException.throwIt(Const.SW_CHANNEL_NOT_SUPPORTED);
        } else if ((p1 & MANAGE_CHANNEL_P1_CLOSE) != 0) {
            byte c = (byte) (p2 & MANAGE_CHANNEL_P2_MASK);
            if (c == 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
            resetChannel(c);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    // Personalization commands (CLA 8X)

    /** Receive a full command data field, even when it exceeds what
     *  setIncomingAndReceive() delivered. */
    private short receiveAll(APDU apdu) {
        short got = apdu.setIncomingAndReceive();
        short full = apdu.getIncomingLength();
        short cdata = apdu.getOffsetCdata();
        while (got < full) {
            short n = apdu.receiveBytes((short) (cdata + got));
            if (n == 0) break;
            got += n;
        }
        return full;
    }

    private static final byte INIT_META_P1_LAST = (byte) 0x80;
    private static final byte INIT_META_P1_OFFSET = 0x7F;

    /** INIT_META: P1 b8 marks the last chunk, P1 b7..b1 || P2 is the
     *  table offset. ADM1 required. */
    private void doInitMeta(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        // A second table would reset every EF and allocate a new data pool
        // without freeing the old one. Reinstall the applet instead.
        if (fs.isMetaLoaded()) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        byte[] buf = apdu.getBuffer();
        short fullLen = receiveAll(apdu);
        short cdata = apdu.getOffsetCdata();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte p2 = buf[ISO7816.OFFSET_P2];
        boolean finished = (p1 & INIT_META_P1_LAST) != 0;
        short offset = (short) (((p1 & INIT_META_P1_OFFSET) << 8) | (p2 & 0xFF));
        if ((short) (offset + fullLen) > fs.metaSize()) {
            ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        }
        fs.initMetaChunk(buf, cdata, offset, fullLen, finished);
    }

    /** PREPERSO UPDATE: P1P2 = FID of a transparent EF, data = content
     *  from offset 0. Writes regardless of the EF's UPDATE condition.
     *  ADM1 required. */
    private void doPrepersoUpdate(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        short fid = Util.getShort(buf, ISO7816.OFFSET_P1);
        Ef ef = fs.find(fid);
        if (ef == null) ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        if (ef.structure != Const.FS_TRANSPARENT) {
            ISOException.throwIt(Const.SW_INCOMPATIBLE_FILE);
        }
        short lc = receiveAll(apdu);
        if (lc == 0 || lc > ef.capacity()) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        Util.arrayCopyNonAtomic(buf, apdu.getOffsetCdata(), ef.pool, ef.poolOffset, lc);
        if (fid == Const.FID_EF_SPC) spcWritten();
    }

    /**
     * Debug: diagnostics for a card whose install or EF access fails.
     * Big-endian shorts: JC API version, max and unused commit
     * capacity, available persistent, transient-reset and
     * transient-deselect memory (32767 means at least that), APDU buffer
     * length, then one byte object-deletion support, one byte EF table
     * loaded, then EF_META and data-pool checksums read by direct
     * indexing and by arrayCopy (FFFF when direct indexing threw), and
     * the data pool length. A hardened build requires ADM1.
     */
    private void doDebugPlatformInfo(APDU apdu, byte channel) {
        if (!Variant.DEBUG) ac.check(Const.AC_ADM, channel);
        short o = 0;
        Util.setShort(scratch, o, JCSystem.getVersion()); o += 2;
        Util.setShort(scratch, o, JCSystem.getMaxCommitCapacity()); o += 2;
        Util.setShort(scratch, o, JCSystem.getUnusedCommitCapacity()); o += 2;
        Util.setShort(scratch, o, JCSystem.getAvailableMemory(JCSystem.MEMORY_TYPE_PERSISTENT)); o += 2;
        Util.setShort(scratch, o, JCSystem.getAvailableMemory(JCSystem.MEMORY_TYPE_TRANSIENT_RESET)); o += 2;
        Util.setShort(scratch, o, JCSystem.getAvailableMemory(JCSystem.MEMORY_TYPE_TRANSIENT_DESELECT)); o += 2;
        Util.setShort(scratch, o, (short) apdu.getBuffer().length); o += 2;
        scratch[o++] = JCSystem.isObjectDeletionSupported() ? (byte) 1 : (byte) 0;
        scratch[o++] = fs.isMetaLoaded() ? (byte) 1 : (byte) 0;
        Util.setShort(scratch, o, checksumDirectSafe(false)); o += 2;
        Util.setShort(scratch, o, fs.checksumCopy(false)); o += 2;
        Util.setShort(scratch, o, checksumDirectSafe(true)); o += 2;
        Util.setShort(scratch, o, fs.checksumCopy(true)); o += 2;
        Util.setShort(scratch, o, fs.poolLength()); o += 2;
        scratch[o++] = buildStage;
        Util.setShort(scratch, o, buildReason); o += 2;
        apdu.setOutgoing();
        apdu.setOutgoingLength(o);
        apdu.sendBytesLong(scratch, (short) 0, o);
    }

    private short checksumDirectSafe(boolean pool) {
        try {
            return fs.checksumDirect(pool);
        } catch (RuntimeException e) {
            return (short) 0xFFFF;
        }
    }

    private static final short DUMP_META_DEFAULT_LE = 32;
    private static final short DUMP_META_MAX_LE = 240;

    private void doDebugDumpMeta(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        short offset = Util.getShort(buf, ISO7816.OFFSET_P1);
        short le = apdu.setOutgoing();
        if (le == 0) le = DUMP_META_DEFAULT_LE;
        if (le > DUMP_META_MAX_LE) le = DUMP_META_MAX_LE;
        short copied = fs.dumpMeta(offset, le, buf, (short) 0);
        apdu.setOutgoingLength(copied);
        apdu.sendBytesLong(buf, (short) 0, copied);
    }

    // Store ESN_MEID_ME (C.S0065-B §9.4.3.1)

    /**
     * P1 = 00: data = ESN length (4) || ESN, lowest-order byte first.
     * P1 = 01: data = MEID length (7) || MEID, lowest-order byte first.
     * The 8 bytes are stored in EF_ESN_ME. Response byte: b1 = 1 if the
     * value differs from what EF_ESN_ME held, b5 = EF_USGIND b1 (UIM_ID
     * used instead of ESN), b6 = EF_USGIND b2 (SF_EUIMID used instead
     * of MEID, P1 = 01 only).
     *
     * Like every CSIM command it needs the CSIM ADF as current directory
     * and PIN verification (C.S0065-B §9.4).
     */
    private void doStoreEsnMeidMe(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        if (p1 != ESN_P1_ESN && p1 != ESN_P1_MEID) {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
        if (buf[ISO7816.OFFSET_P2] != 0) ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        if (lc != ESN_ME_LEN) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        byte idLen = (byte) (buf[ISO7816.OFFSET_CDATA] & 0x0F);
        if (idLen != (p1 == ESN_P1_ESN ? ESN_LEN : MEID_LEN)) {
            ISOException.throwIt(Const.SW_WRONG_DATA);
        }
        Ef ef = fs.find(Const.FID_EF_ESN_ME);
        if (ef == null || ef.capacity() < ESN_ME_LEN) {
            ISOException.throwIt(Const.SW_FILE_NOT_FOUND);
        }
        Util.arrayCopyNonAtomic(ef.pool, ef.poolOffset, esnScratch, (short) 0, ESN_ME_LEN);
        boolean changed = Util.arrayCompare(esnScratch, (short) 0,
                                            buf, ISO7816.OFFSET_CDATA, ESN_ME_LEN) != 0;
        if (changed) write(buf, ISO7816.OFFSET_CDATA, ef.pool, ef.poolOffset, ESN_ME_LEN, channel);

        byte usgind = 0;
        Ef usg = fs.find(Const.FID_EF_USGIND);
        if (usg != null && usg.capacity() >= 1) {
            Util.arrayCopyNonAtomic(usg.pool, usg.poolOffset, esnScratch, (short) 0, (short) 1);
            usgind = esnScratch[0];
        }
        byte resp = changed ? ESN_RESP_CHANGED : 0;
        if ((usgind & USGIND_UIM_ID) != 0) resp |= ESN_RESP_UIM_ID;
        if (p1 == ESN_P1_MEID && (usgind & USGIND_SF_EUIMID) != 0) resp |= ESN_RESP_SF_EUIMID;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 1);
        buf[0] = resp;
        apdu.sendBytes((short) 0, (short) 1);
    }

    // CAVE authentication (C.S0065-B §9.4.1, S.S0053 §2)

    /** Copies EF_IMSI_M into scratch at SC_IMSI. The 24-bit MIN1
     *  (IMSI_M_S1, C.S0065-B §5.2.2 bytes 4-6) and the low byte of MIN2
     *  are read from there. */
    private void readImsiM() {
        Ef ef = fs.find(Const.FID_EF_IMSI_M);
        if (ef == null || ef.capacity() < IMSI_M_LEN) {
            ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        }
        Util.arrayCopyNonAtomic(ef.pool, ef.poolOffset, scratch, SC_IMSI, IMSI_M_LEN);
    }

    /** MIN1 (AUTH_DATA) into scratch[SC_AUTHDATA..+2], most significant
     *  byte first. EF_IMSI_M stores IMSI_M_S1 little-endian in bytes 4-6. */
    private void min1FromImsi() {
        scratch[SC_AUTHDATA] = scratch[(short) (SC_IMSI + 5)];
        scratch[(short) (SC_AUTHDATA + 1)] = scratch[(short) (SC_IMSI + 4)];
        scratch[(short) (SC_AUTHDATA + 2)] = scratch[(short) (SC_IMSI + 3)];
    }

    /**
     * Authenticate - Run CAVE (C.S0065-B §9.4.1.4, P2='80'). Data per
     * C.S0023-D §4.4.4: RANDTYPE, RAND/RANDU, DigLength, DIGIT,
     * Process_Control, ESN. Response is the 3-byte AUTHR/AUTHU. AKA
     * (P2='81'/'82') is not implemented and returns 6A81.
     */
    private void doAuthenticate(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        if (p2 == AUTH_P2_3G_AKA || p2 == AUTH_P2_WLAN_AKA) {
            doAuthenticateAka(apdu, lc, p2 == AUTH_P2_WLAN_AKA);
            return;
        }
        if (p2 != AUTH_P2_RUN_CAVE) ISOException.throwIt(Const.SW_FUNCTION_NOT_SUPPORTED);
        if (lc != RUN_CAVE_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (!cave().canAuthenticate()) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);

        short c = apdu.getOffsetCdata();
        byte randtype = buf[c];
        byte digLen = buf[(short) (c + 5)];
        boolean needImsi = (randtype == RANDTYPE_RANDU) || (digLen == 0);
        if (needImsi) readImsiM();

        if (randtype == RANDTYPE_RAND) {
            Util.arrayCopyNonAtomic(buf, (short) (c + 1), scratch, SC_RAND, (short) 4);
        } else if (randtype == RANDTYPE_RANDU) {
            // RAND_CHALLENGE = RANDU (24 MSBs) || 8 LSBs of IMSI_S2
            // (C.S0005-E §2.3.12.1.4). The low byte of MIN2 is IMSI_M byte 2.
            scratch[SC_RAND] = buf[(short) (c + 2)];
            scratch[(short) (SC_RAND + 1)] = buf[(short) (c + 3)];
            scratch[(short) (SC_RAND + 2)] = buf[(short) (c + 4)];
            scratch[(short) (SC_RAND + 3)] = scratch[(short) (SC_IMSI + 1)];
        } else {
            ISOException.throwIt(Const.SW_WRONG_DATA);
        }

        if (digLen == 0) {
            min1FromImsi();
        } else {
            Util.arrayCopyNonAtomic(buf, (short) (c + 6), scratch, SC_AUTHDATA, (short) 3);
        }

        esnFieldToCave(buf, (short) (c + 10));
        boolean save = (buf[(short) (c + 9)] & PC_SAVE_REGS) != 0;
        cave().runCave(scratch, SC_RAND, scratch, SC_AUTHDATA, scratch, SC_ESN, save, buf, (short) 0);
        apdu.setOutgoing();
        apdu.setOutgoingLength(Cave.SIG_LEN);
        apdu.sendBytes((short) 0, Cave.SIG_LEN);
    }

    private static final short AKA_RANDA_LEN = 16;
    private static final short AKA_AUTN_LEN = 16;
    private static final short AKA_MIN_LC = (short) (AKA_RANDA_LEN + 1 + AKA_AUTN_LEN);
    private static final short UMAC_MAC_LEN = 4;
    private static final byte UMAC_TAG_NO_UAK = 0x00;
    private static final byte UMAC_TAG_SUCCESS = 0x01;
    // UMAC Generation scratch layout: MAC-I in, UMAC out, UAK.
    private static final short SC_UMAC_MACI = 0;
    private static final short SC_UMAC_OUT = 8;
    private static final short SC_UMAC_UAK = 16;
    private static final short AKA_KEY_LEN = 16;
    private static final byte AKA_LOAD_ROOT_KEY = 0x00;
    private static final byte AKA_LOAD_WLAN_KEY = 0x01;
    private static final short CK_IK_LEN = 32;

    /**
     * Authenticate - 3G AKA (C.S0065-B §9.4.1.4, P2='81'). Data per
     * C.S0023-D §4.4.4 P1='01': RANDA(16), L1(1), AUTN(L1). MILENAGE AUTN
     * is 16 bytes. Returns 00||CK||IK||RES_len||RES, or 01||AUTS on a
     * sequence-number resync, or 9804 on a MAC failure.
     */
    private void doAuthenticateAka(APDU apdu, short lc, boolean wlan) {
        byte[] buf = apdu.getBuffer();
        boolean ready = wlan ? aka().isWlanKeyLoaded() : aka().isKeyLoaded();
        if (!ready) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short c = apdu.getOffsetCdata();
        if (lc < AKA_MIN_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        byte l1 = buf[(short) (c + AKA_RANDA_LEN)];
        if (l1 != AKA_AUTN_LEN) ISOException.throwIt(Const.SW_WRONG_DATA);
        byte result = aka().authenticate(wlan, buf, c, buf, (short) (c + AKA_RANDA_LEN + 1), scratch, (short) 0);
        if (result == Aka.RESULT_MAC_FAIL) ISOException.throwIt(Const.SW_AUTH_COMPARE_FAIL);
        short n = (result == Aka.RESULT_SYNC) ? aka().syncResponseLen() : aka().okResponseLen();
        apdu.setOutgoing();
        apdu.setOutgoingLength(n);
        apdu.sendBytesLong(scratch, (short) 0, n);
    }

    /**
     * UMAC Generation (C.S0065-B §9.4.7.1, C.S0023-D §4.12.1). When an AKA
     * run has produced a UAK (f11), this keyed-hashes the 4-byte MAC-I into
     * a UMAC (S.S0078-A §2.1.2.2) and returns Success Tag '01' || UMAC.
     * Without a UAK it returns Success Tag '00'.
     */
    private void doUmacGeneration(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        if (!aka().keysValid()) {
            buf[0] = UMAC_TAG_NO_UAK;
            apdu.setOutgoing();
            apdu.setOutgoingLength((short) 1);
            apdu.sendBytes((short) 0, (short) 1);
            return;
        }
        short c = apdu.getOffsetCdata();
        aka().copyUak(scratch, SC_UMAC_UAK);
        Util.arrayCopyNonAtomic(buf, c, scratch, SC_UMAC_MACI, UMAC_MAC_LEN);
        ss0055().umac(scratch, SC_UMAC_UAK, scratch, SC_UMAC_MACI, UMAC_MAC_LEN, scratch, SC_UMAC_OUT);
        buf[0] = UMAC_TAG_SUCCESS;
        Util.arrayCopyNonAtomic(scratch, SC_UMAC_OUT, buf, (short) 1, UMAC_MAC_LEN);
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) (1 + UMAC_MAC_LEN));
        apdu.sendBytes((short) 0, (short) (1 + UMAC_MAC_LEN));
    }

    /** CONFIRM_KEYS (C.S0065-B §9.4.7.2, C.S0023-D §4.12.2): persist the
     *  CK/IK from the last AKA into EF_3GCIK. No response. */
    private void doConfirmKeys(APDU apdu, byte channel) {
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        if (!aka().keysValid()) ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
        Ef ef = fs.find(Const.FID_EF_3GCIK);
        if (ef == null || ef.capacity() < CK_IK_LEN) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        aka().copyKeys(ef.pool, ef.poolOffset);
    }

    /** Load the AKA 3G root key K (P1='00', 16 bytes) or the WLAN root key
     *  (P1='01', 16 bytes). ADM1 required. */
    private void doLoadAkaKey(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (lc != AKA_KEY_LEN) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (p1 == AKA_LOAD_ROOT_KEY) {
            aka().loadRootKey(buf, c);
        } else if (p1 == AKA_LOAD_WLAN_KEY) {
            aka().loadWlanKey(buf, c);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    /** ESN field (7 bytes, lowest-order byte first, C.S0023-D §4.6) to the
     *  4-byte CAVE ESN at scratch[SC_ESN], most significant byte first. */
    private void esnFieldToCave(byte[] buf, short off) {
        scratch[SC_ESN] = buf[(short) (off + 3)];
        scratch[(short) (SC_ESN + 1)] = buf[(short) (off + 2)];
        scratch[(short) (SC_ESN + 2)] = buf[(short) (off + 1)];
        scratch[(short) (SC_ESN + 3)] = buf[off];
    }

    /**
     * Manage SSD (C.S0065-B §9.4.1.1). P2='00' Update SSD computes the
     * candidate SSD and AUTHBS over the RANDBS from a prior Base Station
     * Challenge. P2='01' Confirm SSD compares the ME's AUTHBS and, unless
     * the OTASP commit-later trigger was set, promotes the candidate.
     */
    private void doManageSsd(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (p2 == SSD_P2_UPDATE) {
            if (lc != UPDATE_SSD_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            if (!cave().isAKeyLoaded()) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
            if (!cave().hasRandBs()) ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
            esnFieldToCave(buf, (short) (c + 8));
            cave().ssdGeneration(buf, c, scratch, SC_ESN);
            readImsiM();
            min1FromImsi();
            cave().computeAuthBs(scratch, SC_AUTHDATA);
            cave().setOtaspPending((buf[(short) (c + 7)] & PC_COMMIT_TRIGGER) != 0);
        } else if (p2 == SSD_P2_CONFIRM) {
            if (lc != CONFIRM_SSD_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            if (!cave().hasSsdCandidate()) ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
            boolean commitNow = !cave().isOtaspPending();
            boolean txn = begin(channel);
            boolean ok = cave().confirmSsd(buf, c, commitNow);
            commit(txn);
            if (!ok) ISOException.throwIt(Const.SW_AUTH_COMPARE_FAIL);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    /**
     * Base Station Challenge (C.S0065-B §9.4.1.2). Generates a fresh
     * 4-byte RANDBS, retains it for the following Update SSD, and returns
     * it. RANDSeed from the ME is not reused (C.S0023-D §4.2.1 permits an
     * independently generated RANDBS).
     */
    private void doBaseStationChallenge(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        if (lc != BS_CHALLENGE_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (rng == null) rng = RandomData.getInstance(RandomData.ALG_SECURE_RANDOM);
        rng.generateData(scratch, SC_RAND, (short) 4);
        cave().setRandBs(scratch, SC_RAND);
        Util.arrayCopyNonAtomic(scratch, SC_RAND, buf, (short) 0, (short) 4);
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 4);
        apdu.sendBytes((short) 0, (short) 4);
    }

    /**
     * Generate Key/VPM (C.S0065-B §9.4.1.3). Produces the 8-byte CMEA key
     * and the requested VPM octets from the registers a prior Run CAVE
     * saved. Data is the first and last VPM octet to return ('FF' 'FF' for
     * none). Returns 9834 when no Run CAVE with save-registers preceded it.
     */
    private void doGenerateKeyVpm(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = apdu.setIncomingAndReceive();
        if (lc != GEN_KEY_LC) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        if (!cave().hasSavedRegisters()) ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
        short c = apdu.getOffsetCdata();
        byte first = buf[c];
        byte last = buf[(short) (c + 1)];
        short vpmLen;
        short f = (short) (first & 0xFF);
        if (first == (byte) 0xFF && last == (byte) 0xFF) {
            vpmLen = 0;
        } else {
            short l = (short) (last & 0xFF);
            if (f > l || l >= Cave.VPM_LEN) ISOException.throwIt(Const.SW_WRONG_DATA);
            vpmLen = (short) (l - f + 1);
        }
        cave().keyVpmGeneration();
        Util.arrayCopyNonAtomic(cave().cmeaKey(), (short) 0, buf, (short) 0, Cave.KEY_LEN);
        if (vpmLen > 0) {
            Util.arrayCopyNonAtomic(cave().vpm(), f, buf, Cave.KEY_LEN, vpmLen);
        }
        short total = (short) (Cave.KEY_LEN + vpmLen);
        apdu.setOutgoing();
        apdu.setOutgoingLength(total);
        apdu.sendBytes((short) 0, total);
    }

    /** Load CAVE secret state (not an EF). P1='00' A-key (8 bytes, zeros
     *  SSD), P1='01' SSD_A || SSD_B (16 bytes). ADM1 required. */
    private void doLoadCaveSecret(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (p1 == CAVE_LOAD_AKEY) {
            if (lc != CAVE_AKEY_LEN) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            cave().loadAKey(buf, c);
        } else if (p1 == CAVE_LOAD_SSD) {
            if (lc != CAVE_SSD_LEN) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            cave().loadSsd(buf, c);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    /** A-key checksum (S.S0053 §2.2.1): the six decimal digits a handset
     *  shows during manual A-key entry. Data is the 4-byte ESN (CAVE
     *  order). Response is the six ASCII digits. ADM1 required. */
    private void doAKeyChecksum(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        short lc = apdu.setIncomingAndReceive();
        if (lc != 4) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        Util.arrayCopyNonAtomic(buf, apdu.getOffsetCdata(), scratch, SC_ESN, (short) 4);
        cave().aKeyChecksum(scratch, SC_ESN, buf, (short) 0);
        apdu.setOutgoing();
        apdu.setOutgoingLength(AKEY_CSUM_DIGITS);
        apdu.sendBytes((short) 0, AKEY_CSUM_DIGITS);
    }

    // OTASP / OTAPA (C.S0065-B §9.4.2)

    /**
     * Generic Key Generation (C.S0065-B §9.4.2.1). P2='00' MS Key Request
     * runs the A-key Diffie-Hellman and returns a result code. P2='01' Key
     * Generation Request returns the result code, MS_RESULT length and
     * MS_RESULT and derives the temporary A-key. Service Key Request
     * (P2='02') is not implemented.
     */
    private void doKeyRequest(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = receiveAll(apdu);
        short c = apdu.getOffsetCdata();
        if (p2 == KEY_P2_MS_KEY) {
            byte rc = otaspDh().msKeyRequest(buf, c, lc);
            buf[0] = rc;
            apdu.setOutgoing();
            apdu.setOutgoingLength((short) 1);
            apdu.sendBytes((short) 0, (short) 1);
        } else if (p2 == KEY_P2_KEY_GEN) {
            short n = otaspDh().keyGenRequest(buf, c, lc, scratch, (short) 0);
            apdu.setOutgoing();
            apdu.setOutgoingLength(n);
            apdu.sendBytesLong(scratch, (short) 0, n);
        } else if (p2 == KEY_P2_SERVICE) {
            doServiceKeyGen(apdu, buf, c);
        } else {
            ISOException.throwIt(Const.SW_FUNCTION_NOT_SUPPORTED);
        }
    }

    // Service-key RAND identifiers (C.S0016-C Table 3.3.10.2-1).
    private static final byte[] SK_RAND_IMS = { (byte) 0x49, (byte) 0x4D, (byte) 0x53 };
    private static final byte[] SK_RAND_BCMCS = { (byte) 0x42, (byte) 0x43, (byte) 0x4D, (byte) 0x43, (byte) 0x53 };
    private static final byte[] SK_RAND_WLAN = { (byte) 0x57, (byte) 0x4C, (byte) 0x41, (byte) 0x4E };

    /**
     * Service Key Generation Request (C.S0065-B §9.4.2.1 P2='02', C.S0016-C
     * §3.3.10). Derives ITM_RAND = f0(RKP) and, for each requested bit in
     * the KEY_ID bitmap, a 128-bit service root key = f3(ITM_RAND, RAND).
     * The WLAN root key feeds WLAN AKA. Response is a one-byte result code.
     */
    private void doServiceKeyGen(APDU apdu, byte[] buf, short c) {
        if (!aka().isKeyLoaded()) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        byte keyId = buf[c];
        aka().copyRootKeyK(scratch, (short) 0);
        ss0055().serviceItmRand(scratch, (short) 0, scratch, (short) 16);   // ITM_RAND at [16]
        if ((keyId & 0x04) != 0) {   // WLAN Root Key
            ss0055().serviceKey(scratch, (short) 16, SK_RAND_WLAN, (short) 0, (short) 4, scratch, (short) 32);
            aka().loadWlanKey(scratch, (short) 32);
        }
        if ((keyId & 0x01) != 0) {   // IMS Root Key (computed, no on-card consumer)
            ss0055().serviceKey(scratch, (short) 16, SK_RAND_IMS, (short) 0, (short) 3, scratch, (short) 48);
        }
        if ((keyId & 0x02) != 0) {   // BCMCS Root Key
            ss0055().serviceKey(scratch, (short) 16, SK_RAND_BCMCS, (short) 0, (short) 5, scratch, (short) 48);
        }
        buf[0] = VAL_RESULT_ACCEPT;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 1);
        apdu.sendBytes((short) 0, (short) 1);
    }

    /**
     * Commit (C.S0065-B §9.4.2.2). Promotes the temporary A-key from the
     * key exchange and the candidate SSD into permanent storage. Response
     * is a one-byte result code.
     */
    private void doCommit(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        if (otaLocked()) {
            buf[0] = VAL_RESULT_LOCKED;
            apdu.setOutgoing();
            apdu.setOutgoingLength((short) 1);
            apdu.sendBytes((short) 0, (short) 1);
            return;
        }
        boolean txn = begin(channel);
        if (otaspDh().hasAKeyTemp()) {
            otaspDh().copyAKeyTemp(scratch, (short) 0);
            cave().loadAKey(scratch, (short) 0);
        }
        if (otaspDh().hasRootKey()) {
            otaspDh().copyRootKey(scratch, (short) 0);
            aka().loadRootKey(scratch, (short) 0);
        }
        cave().commitSsd();
        applyStagedBlock();
        commit(txn);
        buf[0] = VAL_RESULT_ACCEPT;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 1);
        apdu.sendBytes((short) 0, (short) 1);
    }

    /**
     * Validate (C.S0065-B §9.4.2.3, C.S0016-C §4.5.4). Block ID '00'
     * Verify SPC compares the supplied SPC against EF_SPC, '01' Change SPC
     * writes a new SPC once verified, '02' Validate SPASM. Response is the
     * Block ID and a result code.
     */
    private void doValidate(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = receiveAll(apdu);
        short c = apdu.getOffsetCdata();
        if (lc < 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        byte blockId = buf[c];
        short paramOff = (short) (c + 2);
        byte result;
        switch (blockId) {
            case VAL_BLOCK_VERIFY_SPC:
                result = verifySpc(buf, paramOff);
                otaFlags[OTA_SPC_VERIFIED] = (result == VAL_RESULT_ACCEPT) ? (byte) 1 : 0;
                break;
            case VAL_BLOCK_CHANGE_SPC:
                if (spLocked()) {
                    result = VAL_RESULT_LOCKED;
                } else {
                    boolean txn = begin(channel);
                    writeSpc(buf, paramOff);
                    commit(txn);
                    result = VAL_RESULT_ACCEPT;
                }
                break;
            case VAL_BLOCK_SPASM:
                result = validateSpasm(buf, paramOff);
                break;
            default:
                result = VAL_RESULT_REJECT;
        }
        buf[0] = blockId;
        buf[1] = result;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 2);
        apdu.sendBytes((short) 0, (short) 2);
    }

    /** Verify SPC (C.S0016-C §3.5.1.7). Accepted when the 3-byte SPC
     *  matches EF_SPC or EF_SPC is '00 00 00' (C.S0016-C §3.3.6). After
     *  SPC_MAX_TRIES mismatches every attempt is "Mobile station locked"
     *  until EF_SPC is written again. */
    private byte verifySpc(byte[] buf, short off) {
        Ef ef = fs.find(Const.FID_EF_SPC);
        if (ef == null || ef.capacity() < SPC_LEN) return VAL_RESULT_REJECT;
        if (spcIsDefault()) return VAL_RESULT_ACCEPT;
        if (spcFails >= SPC_MAX_TRIES) return VAL_RESULT_LOCKED;
        spcFails++;
        if (Util.arrayCompare(ef.pool, ef.poolOffset, buf, off, SPC_LEN) != 0) {
            return VAL_RESULT_INVALID_SPC;
        }
        spcFails = 0;
        return VAL_RESULT_ACCEPT;
    }

    private void writeSpc(byte[] buf, short off) {
        Ef ef = fs.find(Const.FID_EF_SPC);
        if (ef == null || ef.capacity() < SPC_LEN) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        Util.arrayCopy(buf, off, ef.pool, ef.poolOffset, SPC_LEN);
        spcWritten();
    }

    /** A new EF_SPC value updates EF_SPCS and clears the failed-attempt
     *  count. */
    private void spcWritten() {
        fs.syncSpcStatus();
        if (spcFails != 0) spcFails = 0;
    }

    /** True when EF_SPC is '00 00 00', the default value that leaves
     *  SP_LOCK_STATE at 0 (C.S0016-C §3.2.1). */
    private boolean spcIsDefault() {
        Ef ef = fs.find(Const.FID_EF_SPC);
        if (ef == null || ef.capacity() < SPC_LEN) return false;
        return (ef.pool[ef.poolOffset] | ef.pool[(short) (ef.poolOffset + 1)]
                | ef.pool[(short) (ef.poolOffset + 2)]) == 0;
    }

    /** SP_LOCK_STATE (C.S0016-C §3.2.1). */
    private boolean spLocked() {
        return otaFlags[OTA_SPC_VERIFIED] == 0 && !spcIsDefault();
    }

    /** SP_LOCK_STATE or NAM_LOCK_STATE set, which rejects Configuration,
     *  Download and Commit with "Mobile station locked" (C.S0016-C §3.5). */
    private boolean otaLocked() {
        return spLocked() || otaFlags[OTA_NAM_LOCKED] != 0;
    }

    /**
     * Validate SPASM (C.S0016-C §4.5.4.3). Parses AUTH_OTAPA_INCL and the
     * 18-bit AUTH_OTAPA from the bit-packed parameter block and compares
     * it to the AUTH_OTAPA computed when the OTAPA session opened.
     */
    private byte validateSpasm(byte[] buf, short off) {
        if (otaFlags[OTA_NAM_LOCKED] == 0) return VAL_RESULT_ACCEPT;
        short b0 = (short) (buf[off] & 0xFF);
        short b1 = (short) (buf[(short) (off + 1)] & 0xFF);
        short b2 = (short) (buf[(short) (off + 2)] & 0xFF);
        if ((b0 & 0x80) == 0) return VAL_RESULT_INVALID_SPASM;   // AUTH_OTAPA not included
        // AUTH_OTAPA (18 bits) begins at field bit 1, so bits 17..16 are in
        // b0 b7..b6, repacked into the 3-byte form the engine produces.
        byte r0 = (byte) ((b0 >> 5) & 0x03);
        byte r1 = (byte) (((b0 & 0x1F) << 3) | ((b1 >> 5) & 0x07));
        byte r2 = (byte) (((b1 & 0x1F) << 3) | ((b2 >> 5) & 0x07));
        if (r0 != otaAuthOtapa[0] || r1 != otaAuthOtapa[1] || r2 != otaAuthOtapa[2]) {
            return VAL_RESULT_INVALID_SPASM;
        }
        otaFlags[OTA_NAM_LOCKED] = 0;
        return VAL_RESULT_ACCEPT;
    }

    /**
     * Generic Download Request (C.S0065-B §9.4.2.5). P2='00' stages a NAM
     * parameter block (Block ID, Block Length, Param Data) in the
     * temporary NAM for Commit. P2='01' is an SSPR PRL download: the
     * param data is SEGMENT_OFFSET(2) followed by a PRL segment, written
     * straight into EF_EPRL and marked pending until Commit activates it
     * (the shadow-copy / flag scheme that avoids a 12 kB atomic commit).
     * Response is Block ID and a result code.
     */
    private void doDownloadRequest(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = receiveAll(apdu);
        short c = apdu.getOffsetCdata();
        if (lc < 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        byte blockId = buf[c];
        short dataLen = (short) (buf[(short) (c + 1)] & 0xFF);
        byte result = VAL_RESULT_ACCEPT;
        short famFid = downloadFid(p2);
        if (otaLocked()) {
            result = VAL_RESULT_LOCKED;
        } else if ((short) (2 + dataLen) > lc) {
            result = VAL_RESULT_REJECT;
        } else if (p2 == DL_P2_NAM) {
            secureParamData(buf, (short) (c + 2), dataLen);   // decrypt if Secure Mode
            result = stageBlock(blockId, buf, (short) (c + 2), dataLen);
        } else if (famFid != 0) {
            secureParamData(buf, (short) (c + 2), dataLen);
            result = downloadSegment(famFid, buf, (short) (c + 2), dataLen, channel);
        } else {
            result = VAL_RESULT_REJECT;
        }
        buf[0] = blockId;
        buf[1] = result;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 2);
        apdu.sendBytes((short) 0, (short) 2);
    }

    /** Hold a NAM block in a free or matching staging slot. */
    private byte stageBlock(byte blockId, byte[] buf, short off, short dataLen) {
        if (dataLen > STAGE_SLOT_DATA) return VAL_RESULT_REJECT;
        short slot = -1;
        short free = -1;
        for (short i = 0; i < STAGE_SLOTS; i++) {
            short base = (short) (i * STAGE_SLOT);
            if (stage[base] != 0 && stage[(short) (base + 1)] == blockId) { slot = base; break; }
            if (stage[base] == 0 && free < 0) free = base;
        }
        if (slot < 0) slot = free;
        if (slot < 0) return VAL_RESULT_REJECT;
        stage[slot] = 1;
        stage[(short) (slot + 1)] = blockId;
        stage[(short) (slot + 2)] = (byte) dataLen;
        Util.arrayCopyNonAtomic(buf, off, stage, (short) (slot + STAGE_HDR), dataLen);
        return VAL_RESULT_ACCEPT;
    }

    /** Download one parameter-block segment, SEGMENT_OFFSET(2) then bytes,
     *  straight into the family's EF (C.S0016 SSPR/PUZL/3GPD/MMS/System
     *  Tag, the shadow-copy / flag scheme). */
    private byte downloadSegment(short fid, byte[] buf, short off, short dataLen, byte channel) {
        if (dataLen < 2) return VAL_RESULT_REJECT;
        short segOff = Util.getShort(buf, off);
        short segLen = (short) (dataLen - 2);
        Ef ef = fs.find(fid);
        if (ef == null || (short) (segOff + segLen) > ef.capacity()) return VAL_RESULT_REJECT;
        write(buf, (short) (off + 2), ef.pool, (short) (ef.poolOffset + segOff), segLen, channel);
        otaFlags[OTA_PRL_PENDING] = 1;
        return VAL_RESULT_ACCEPT;
    }

    /** EF written by a Download Request of family P2 (C.S0065-B §9.4.2.5):
     *  SSPR to EF_EPRL, PUZL, 3GPD, MMS and System Tag to their EFs. */
    private short downloadFid(byte p2) {
        switch (p2) {
            case CFG_P2_SSPR:      return Const.FID_EF_EPRL;
            case CFG_P2_PUZL:      return Const.FID_EF_PUZL;
            case CFG_P2_3GPD:      return Const.FID_EF_3GPDOPM;
            case CFG_P2_MMS:       return Const.FID_EF_MMSUP;
            case CFG_P2_SYSTAG:    return Const.FID_EF_HOME_TAG;
            default:               return 0;
        }
    }

    /** EF read by a Configuration Request of family P2: SSPR returns the
     *  PRL dimensions (EF_MAXPRL), the rest return their block EF. */
    private short configFid(byte p2) {
        switch (p2) {
            case CFG_P2_SSPR:      return Const.FID_EF_MAXPRL;
            case CFG_P2_PUZL:      return Const.FID_EF_PUZL;
            case CFG_P2_3GPD:      return Const.FID_EF_3GPDOPM;
            case CFG_P2_MMS:       return Const.FID_EF_MMSUP;
            case CFG_P2_SYSTAG:    return Const.FID_EF_HOME_TAG;
            default:               return 0;
        }
    }

    /** Configuration Response for a rejected block: BLOCK_LEN 0. */
    private static void sendConfigRejected(APDU apdu, byte blockId, byte result) {
        byte[] buf = apdu.getBuffer();
        buf[0] = blockId;
        buf[1] = 0;
        buf[2] = result;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 3);
        apdu.sendBytes((short) 0, (short) 3);
    }

    /**
     * Generic Configuration Request (C.S0065-B §9.4.2.4). P2='00' returns
     * a NAM block built from the EFs when the applet can encode it, else
     * the staged copy. Response is Block ID, Block Length, Result Code and
     * Param Data.
     */
    private void doConfigRequest(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (lc < 1) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        byte blockId = buf[c];
        if (otaLocked()) {
            sendConfigRejected(apdu, blockId, VAL_RESULT_LOCKED);
            return;
        }
        short dataLen;
        if (p2 == CFG_P2_NAM) {
            dataLen = otaNam().encode(blockId, scratch, (short) 0); // NAM blocks from EFs
        } else {
            short fid = configFid(p2);
            dataLen = (fid != 0) ? efToConfig(fid, scratch) : (short) -1;
        }
        if (dataLen < 0) dataLen = stagedCopy(blockId, scratch, (short) 0);
        if (dataLen < 0) {
            sendConfigRejected(apdu, blockId, VAL_RESULT_REJECT);
            return;
        }
        secureParamData(scratch, (short) 0, dataLen);   // encrypt if Secure Mode
        buf[0] = blockId;
        buf[1] = (byte) dataLen;
        buf[2] = VAL_RESULT_ACCEPT;
        Util.arrayCopyNonAtomic(scratch, (short) 0, buf, (short) 3, dataLen);
        short total = (short) (3 + dataLen);
        apdu.setOutgoing();
        apdu.setOutgoingLength(total);
        apdu.sendBytes((short) 0, total);
    }

    /** Return an EF's whole content as a Config block's param data, or -1
     *  if the EF is absent. Used for SSPR PRL dimensions and 3GPD. */
    private short efToConfig(short fid, byte[] out) {
        Ef ef = fs.find(fid);
        if (ef == null) return -1;
        short n = ef.capacity();
        if (n > STAGE_SLOT_DATA) n = STAGE_SLOT_DATA;
        Util.arrayCopyNonAtomic(ef.pool, ef.poolOffset, out, (short) 0, n);
        return n;
    }

    /** Copy a staged block's data to out, or return -1 if not staged. */
    private short stagedCopy(byte blockId, byte[] out, short off) {
        for (short i = 0; i < STAGE_SLOTS; i++) {
            short base = (short) (i * STAGE_SLOT);
            if (stage[base] != 0 && stage[(short) (base + 1)] == blockId) {
                short n = (short) (stage[(short) (base + 2)] & 0xFF);
                Util.arrayCopyNonAtomic(stage, (short) (base + STAGE_HDR), out, off, n);
                return n;
            }
        }
        return -1;
    }

    /** Move the temporary NAM into the EFs at Commit: decode every staged
     *  block to its EFs and activate a pending PRL. */
    private void applyStagedBlock() {
        for (short i = 0; i < STAGE_SLOTS; i++) {
            short base = (short) (i * STAGE_SLOT);
            if (stage[base] == 0) continue;
            short n = (short) (stage[(short) (base + 2)] & 0xFF);
            otaNam().decode(stage[(short) (base + 1)], stage, (short) (base + STAGE_HDR), n);
            stage[base] = 0;
        }
        otaFlags[OTA_PRL_PENDING] = 0;   // PRL segments were written in place
    }

    /**
     * Secure Mode (C.S0065-B §9.4.2.7). P1='00' starts secure mode with
     * RAND_SM(8). P1='01' stops it, P2 = KEY_IN_USE. The command state is
     * tracked here. ECMEA (S.S0055) encryption of the subsequent parameter
     * blocks is the documented extension point. Response is a result code.
     */
    private void doSecureMode(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = apdu.setIncomingAndReceive();
        if (p1 == SM_P1_START) {
            if (lc < 8) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            short c = apdu.getOffsetCdata();
            // K = SSD_A||SSD_B at scratch[0..15], RAND = RAND_SM||RAND_SM at [16..31].
            cave().copySsdAB(scratch, (short) 0);
            Util.arrayCopyNonAtomic(buf, c, scratch, (short) 16, (short) 8);
            Util.arrayCopyNonAtomic(buf, c, scratch, (short) 24, (short) 8);
            ss0055().f3(scratch, (short) 0, scratch, (short) 16, scratch, (short) 32);  // SMCK
            ss0055().espSetKey(scratch, (short) 32);
            secMode[0] = 1;
        } else {
            secMode[0] = 0;
        }
        buf[0] = VAL_RESULT_ACCEPT;
        apdu.setOutgoing();
        apdu.setOutgoingLength((short) 1);
        apdu.sendBytes((short) 0, (short) 1);
    }

    /** ESP-encrypt or decrypt (XOR keystream) a parameter-data span when
     *  Secure Mode is active, keyed by SMCK and the current FRESH. */
    private void secureParamData(byte[] buf, short off, short len) {
        if (secMode[0] == 0 || len == 0) return;
        ss0055().espMask(fresh, (short) 1, (short) 2, buf, off, len);
    }

    /**
     * FRESH (C.S0065-B §9.4.2.8). P1='00' stores the 2-byte Crypto-Sync.
     * P1='01' returns it.
     */
    private void doFresh(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (p1 == FRESH_P1_PUT) {
            if (lc < 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
            fresh[0] = 1;
            fresh[1] = buf[c];
            fresh[2] = buf[(short) (c + 1)];
        } else if (p1 == FRESH_P1_GET) {
            if (fresh[0] == 0) ISOException.throwIt(Const.SW_AUTH_COMPARE_FAIL);
            buf[0] = fresh[1];
            buf[1] = fresh[2];
            apdu.setOutgoing();
            apdu.setOutgoingLength((short) 2);
            apdu.sendBytes((short) 0, (short) 2);
        } else {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    private boolean otapaEnabled() {
        Ef ef = fs.find(Const.FID_EF_OTAPASPC);
        if (ef == null || ef.capacity() < 1) return false;
        Util.arrayCopyNonAtomic(ef.pool, ef.poolOffset, esnScratch, (short) 0, (short) 1);
        return (esnScratch[0] & 0x01) != 0;
    }

    /**
     * OTAPA Request (C.S0065-B §9.4.2.6). Start (b1=1) opens a network
     * programming session: when the active NAM is SPASM-protected it
     * returns a fresh RAND_OTAPA and retains the AUTH_OTAPA the network
     * must echo in a Validate SPASM. Stop (b1=0) closes the session. Data
     * is Start/Stop, RANDSeed(4) and ESN(7). Response is Result Code,
     * NAM_LOCK Indicator and (when locked) RAND_OTAPA.
     */
    private void doOtapaRequest(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        short lc = apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (lc < 12) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        boolean start = (buf[c] & OTAPA_START) != 0;
        boolean namLock = otapaEnabled();
        if (start && namLock) {
            if (rng == null) rng = RandomData.getInstance(RandomData.ALG_SECURE_RANDOM);
            rng.generateData(scratch, SC_RAND, (short) 4);
            readImsiM();
            min1FromImsi();
            esnFieldToCave(buf, (short) (c + 5));
            cave().authOtapa(scratch, SC_RAND, scratch, SC_AUTHDATA, scratch, SC_ESN,
                           otaAuthOtapa, (short) 0);
            otaFlags[OTA_OTAPA_ACTIVE] = 1;
            otaFlags[OTA_NAM_LOCKED] = 1;
            buf[0] = VAL_RESULT_ACCEPT;
            buf[1] = 0x01;
            Util.arrayCopyNonAtomic(scratch, SC_RAND, buf, (short) 2, (short) 4);
            apdu.setOutgoing();
            apdu.setOutgoingLength(OTAPA_RESP_WITH_RAND);
            apdu.sendBytes((short) 0, OTAPA_RESP_WITH_RAND);
            return;
        }
        if (!start) {
            otaFlags[OTA_OTAPA_ACTIVE] = 0;
            otaFlags[OTA_NAM_LOCKED] = 0;
        }
        buf[0] = VAL_RESULT_ACCEPT;
        buf[1] = namLock ? (byte) 0x01 : 0x00;
        apdu.setOutgoing();
        apdu.setOutgoingLength(OTAPA_RESP_NO_RAND);
        apdu.sendBytes((short) 0, OTAPA_RESP_NO_RAND);
    }

    // Packet-data IP authentication (C.S0065-B §9.4.4, C.S0023-D §4.8.1)

    /**
     * Compute IP Authentication. P1 selects CHAP (00), MN-HA (01),
     * MIP-RRQ Hash (02), MN-AAA (03) or HRPD access (04). MN-HA and
     * MIP-RRQ chain their registration data over several blocks via P2.
     */
    private void doComputeIpAuth(APDU apdu, byte channel) {
        byte[] buf = apdu.getBuffer();
        byte p1 = buf[ISO7816.OFFSET_P1];
        byte p2 = buf[ISO7816.OFFSET_P2];
        if (dfOfChannel[channel] != DF_ADF) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        ac.check(Const.AC_PIN1, channel);
        short lc = receiveAll(apdu);
        short c = apdu.getOffsetCdata();
        switch (p1) {
            case IP_P1_CHAP: {
                if (lc < 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                ipAuth().chap(IpAuth.CLASS_SIP_CHAP, buf[c], buf[(short) (c + 1)],
                            buf, (short) (c + 2), (short) (lc - 2), scratch, (short) 0);
                sendHash(apdu);
                break;
            }
            case IP_P1_HRPD: {
                if (lc < 1) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                ipAuth().chap(IpAuth.CLASS_HRPD, buf[c], (byte) 0,
                            buf, (short) (c + 1), (short) (lc - 1), scratch, (short) 0);
                sendHash(apdu);
                break;
            }
            case IP_P1_MN_HA: {
                boolean first = (p2 == IP_BLK_FIRST) || (p2 == IP_BLK_SINGLE);
                boolean last = (p2 == IP_BLK_SINGLE) || (p2 == IP_BLK_LAST);
                byte nai = 0;
                short off = c;
                short dlen = lc;
                if (first) {
                    if (lc < 1) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                    nai = buf[c];
                    off = (short) (c + 1);
                    dlen = (short) (lc - 1);
                } else if (p2 != IP_BLK_NEXT && p2 != IP_BLK_LAST) {
                    ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                }
                ipAuth().mnHaBlock(first, last, nai, buf, off, dlen, scratch, (short) 0);
                if (last) sendHash(apdu);
                break;
            }
            case IP_P1_MIP_RRQ: {
                boolean first = (p2 == IP_BLK_FIRST) || (p2 == IP_BLK_SINGLE);
                boolean last = (p2 == IP_BLK_SINGLE) || (p2 == IP_BLK_LAST);
                if (!first && p2 != IP_BLK_NEXT && p2 != IP_BLK_LAST) {
                    ISOException.throwIt(Const.SW_INCORRECT_P1P2);
                }
                ipAuth().mipRrqBlock(first, last, buf, c, lc);
                break;
            }
            case IP_P1_MN_AAA: {
                if (lc < 2) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
                ipAuth().mnAaa(buf[c], buf, (short) (c + 1), (short) (lc - 1), scratch, (short) 0);
                sendHash(apdu);
                break;
            }
            default:
                ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
    }

    /** Provision a packet-data shared secret (ADM). P1 = class, P2 =
     *  NAI-Entry-Index, data = the shared secret. */
    private void doLoadIpSecret(APDU apdu, byte channel) {
        ac.check(Const.AC_ADM, channel);
        byte[] buf = apdu.getBuffer();
        byte cls = buf[ISO7816.OFFSET_P1];
        byte idx = buf[ISO7816.OFFSET_P2];
        short lc = receiveAll(apdu);
        if (lc == 0) ISOException.throwIt(ISO7816.SW_WRONG_LENGTH);
        ipAuth().loadSecret(cls, idx, buf, apdu.getOffsetCdata(), lc);
    }

    /**
     * Debug: evaluate the S.S0055 kernel on caller-supplied inputs so the
     * self-test suite can check it against the published vectors. P1 picks
     * f0 (seed||counter), f1 (K||RAND||SQN||AMF), f3 (K||RAND) or ESP
     * (key||freshLen||fresh||len||data). It operates only on inputs in the
     * command, never on stored secrets. A hardened build answers 6D00.
     */
    private void doCryptoSelfTest(APDU apdu) {
        byte[] b = apdu.getBuffer();
        byte p1 = b[ISO7816.OFFSET_P1];
        apdu.setIncomingAndReceive();
        short c = apdu.getOffsetCdata();
        if (p1 == 0) {
            ss0055().f0(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 8);
        } else if (p1 == 1) {
            ss0055().f1(b, c, b, (short) (c + 16), b, (short) (c + 32), b, (short) (c + 38), scratch, (short) 0);
            sendN(apdu, (short) 8);
        } else if (p1 == 2) {
            ss0055().f3(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 16);
        } else if (p1 == 4) {
            ss0055().f1star(b, c, b, (short) (c + 16), b, (short) (c + 32), b, (short) (c + 38), scratch, (short) 0);
            sendN(apdu, (short) 8);
        } else if (p1 == 5) {
            ss0055().f2(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 16);
        } else if (p1 == 6) {
            ss0055().f4(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 16);
        } else if (p1 == 7) {
            ss0055().f5(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 6);
        } else if (p1 == 8) {
            ss0055().f5star(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 6);
        } else if (p1 == 9) {
            ss0055().f11(b, c, b, (short) (c + 16), scratch, (short) 0);
            sendN(apdu, (short) 16);
        } else if (p1 == 3) {
            short fl = (short) (b[(short) (c + 16)] & 0xFF);
            short fo = (short) (c + 17);
            short nb = (short) (b[(short) (fo + fl)] & 0xFF);
            short bo = (short) (fo + fl + 1);
            ss0055().espSetKey(b, c);
            Util.arrayCopyNonAtomic(b, bo, scratch, (short) 0, nb);
            ss0055().espMask(b, fo, fl, scratch, (short) 0, nb);
            sendN(apdu, nb);
        }
    }

    private void sendN(APDU apdu, short n) {
        byte[] bb = apdu.getBuffer();
        Util.arrayCopyNonAtomic(scratch, (short) 0, bb, (short) 0, n);
        apdu.setOutgoing();
        apdu.setOutgoingLength(n);
        apdu.sendBytes((short) 0, n);
    }

    /** Send the 16-byte hash held at scratch[0] as the response. */
    private void sendHash(APDU apdu) {
        byte[] buf = apdu.getBuffer();
        Util.arrayCopyNonAtomic(scratch, (short) 0, buf, (short) 0, IpAuth.HASH_LEN);
        apdu.setOutgoing();
        apdu.setOutgoingLength(IpAuth.HASH_LEN);
        apdu.sendBytes((short) 0, IpAuth.HASH_LEN);
    }

    // helpers

    private byte channelOf(APDU apdu) {
        byte ch = apdu.getCLAChannel();
        if (ch < 0 || ch >= FileSystem.MAX_CHANNELS) {
            ISOException.throwIt(Const.SW_CHANNEL_NOT_SUPPORTED);
        }
        return ch;
    }

    private Ef currentEf(byte channel) {
        Ef ef = fs.current(channel);
        if (ef == null) ISOException.throwIt(Const.SW_NO_EF_SELECTED);
        return ef;
    }
}
