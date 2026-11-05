package org.onexbts.csim;

import javacard.framework.ISOException;
import javacard.framework.JCSystem;
import javacard.framework.Util;
import javacard.security.MessageDigest;

/**
 * Packet-data IP authentication (C.S0065-B §9.4.4, C.S0023-D §4.8.1):
 * CHAP for Simple IP and HRPD access, and the MN-HA and MN-AAA
 * authenticators for Mobile IP. All five use a one-way hash. MD5 is the
 * defined function (C.S0023-D §4.8.1.x, RFC 1994 / RFC 3012).
 *
 *   CHAP / HRPD   = MD5(CHAP-ID || CHAP-SS || CHAP-Challenge)
 *   MN-HA         = MD5(MN-HA SS || Registration-Message || MN-HA SS)
 *   MIP-RRQ Hash  = MD5(Preceding-MIP-RRQ || MN-AAA Extension Header)
 *   MN-AAA        = MD5(high byte || MN-AAA SS || MIP-RRQ Hash || challenge)
 *
 * MN-HA and MIP-RRQ accept the registration data in chained blocks. The
 * MIP-RRQ hash is retained for the following MN-AAA computation.
 *
 * Shared secrets are not derived from the 3GPD EFs here. They are
 * provisioned directly by an ADM command into per-class, per-NAI-index
 * slots. Production provisioning fills these from the operator's SIP/MIP
 * user-profile data.
 */
public final class IpAuth {

    public static final short HASH_LEN = 16;        // MD5 output
    public static final byte CLASS_SIP_CHAP = 0;
    public static final byte CLASS_MN_HA = 1;
    public static final byte CLASS_MN_AAA = 2;
    public static final byte CLASS_HRPD = 3;
    private static final byte NUM_CLASSES = 4;
    private static final byte NUM_INDEXES = 4;
    private static final short SLOT = 32;           // max shared-secret length
    private static final byte NUM_SLOTS = (byte) (NUM_CLASSES * NUM_INDEXES);

    // Chaining operation currently open (P1 of Compute IP Authentication).
    private static final byte CHAIN_NONE = -1;

    // Persistent shared-secret store: NUM_SLOTS x SLOT, with a length per slot.
    private final byte[] ssPool;
    private final byte[] ssLen;

    private final MessageDigest md;

    // Transient chaining state for MN-HA / MIP-RRQ.
    private final byte[] mipRrqHash;    // 16, last MIP-RRQ Hash result
    private final byte[] haSs;          // 32, MN-HA secret kept for the trailer
    private final byte[] st;            // [0]=chainOp [1]=haSsLen [2]=mipRrqValid
    private static final short ST_OP = 0;
    private static final short ST_HA_LEN = 1;
    private static final short ST_MIP_VALID = 2;

    public IpAuth() {
        ssPool = new byte[(short) (NUM_SLOTS * SLOT)];
        ssLen = new byte[NUM_SLOTS];
        md = MessageDigest.getInstance(MessageDigest.ALG_MD5, false);
        mipRrqHash = JCSystem.makeTransientByteArray(HASH_LEN, JCSystem.CLEAR_ON_DESELECT);
        haSs = JCSystem.makeTransientByteArray(SLOT, JCSystem.CLEAR_ON_DESELECT);
        st = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        st[ST_OP] = CHAIN_NONE;
    }

    private static short slotIndex(byte cls, byte idx) {
        if (cls < 0 || cls >= NUM_CLASSES || idx < 0 || idx >= NUM_INDEXES) {
            ISOException.throwIt(Const.SW_INCORRECT_P1P2);
        }
        return (short) ((cls * NUM_INDEXES + idx) * SLOT);
    }

    /** Provision a shared secret (ADM). */
    public void loadSecret(byte cls, byte idx, byte[] src, short off, short len) {
        if (len > SLOT) ISOException.throwIt(Const.SW_WRONG_DATA);
        short base = slotIndex(cls, idx);
        Util.arrayFillNonAtomic(ssPool, base, SLOT, (byte) 0);
        Util.arrayCopyNonAtomic(src, off, ssPool, base, len);
        ssLen[(byte) (cls * NUM_INDEXES + idx)] = (byte) len;
    }

    private short ssOffset(byte cls, byte idx) {
        return slotIndex(cls, idx);
    }

    /** 6985 when no secret is loaded in the slot, so an authenticator is
     *  never computed over an empty secret. */
    private short ssLength(byte cls, byte idx) {
        short n = (short) (ssLen[(byte) (cls * NUM_INDEXES + idx)] & 0xFF);
        if (n == 0) ISOException.throwIt(Const.SW_CONDITIONS_NOT_SAT);
        return n;
    }

    // CHAP (Simple IP, P1=00) and HRPD access (P1=04)

    /** MD5(CHAP-ID || CHAP-SS || CHAP-Challenge) into out[outOff..+15]. */
    public void chap(byte cls, byte chapId, byte naiIdx,
                     byte[] chal, short chalOff, short chalLen,
                     byte[] out, short outOff) {
        short ssOff = ssOffset(cls, naiIdx);
        short ssN = ssLength(cls, naiIdx);
        md.reset();
        haSs[0] = chapId;
        md.update(haSs, (short) 0, (short) 1);
        md.update(ssPool, ssOff, ssN);
        md.doFinal(chal, chalOff, chalLen, out, outOff);
    }

    // MN-HA authenticator (P1=01), chained

    /** Begin / continue the MN-HA chain. firstBlock keeps the SS for the
     *  trailing copy. lastBlock finalizes MD5(SS||RegMsg||SS). */
    public void mnHaBlock(boolean firstBlock, boolean lastBlock, byte naiIdx,
                          byte[] data, short dataOff, short dataLen,
                          byte[] out, short outOff) {
        if (firstBlock) {
            short ssOff = ssOffset(CLASS_MN_HA, naiIdx);
            short ssN = ssLength(CLASS_MN_HA, naiIdx);
            md.reset();
            md.update(ssPool, ssOff, ssN);
            Util.arrayCopyNonAtomic(ssPool, ssOff, haSs, (short) 0, ssN);
            st[ST_HA_LEN] = (byte) ssN;
            st[ST_OP] = CLASS_MN_HA;
        } else if (st[ST_OP] != CLASS_MN_HA) {
            ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
        }
        if (lastBlock) {
            md.update(data, dataOff, dataLen);
            md.doFinal(haSs, (short) 0, (short) (st[ST_HA_LEN] & 0xFF), out, outOff);
            st[ST_OP] = CHAIN_NONE;
        } else {
            md.update(data, dataOff, dataLen);
        }
    }

    // MIP-RRQ Hash (P1=02), chained. Result kept for MN-AAA

    public void mipRrqBlock(boolean firstBlock, boolean lastBlock,
                            byte[] data, short dataOff, short dataLen) {
        if (firstBlock) {
            md.reset();
            st[ST_OP] = CLASS_MN_AAA;   // reuse marker for the MIP-RRQ chain
        } else if (st[ST_OP] != CLASS_MN_AAA) {
            ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
        }
        if (lastBlock) {
            md.doFinal(data, dataOff, dataLen, mipRrqHash, (short) 0);
            st[ST_MIP_VALID] = 1;
            st[ST_OP] = CHAIN_NONE;
        } else {
            md.update(data, dataOff, dataLen);
        }
    }

    // MN-AAA authenticator (P1=03)

    /** MD5(highByte || MN-AAA SS || MIP-RRQ Hash || challenge). Requires a
     *  prior MIP-RRQ Hash. The high-order byte of the challenge leads and
     *  the whole challenge follows (C.S0023-D §4.8.1.4). */
    public void mnAaa(byte naiIdx, byte[] chal, short chalOff, short chalLen,
                      byte[] out, short outOff) {
        if (st[ST_MIP_VALID] == 0) ISOException.throwIt(Const.SW_CAVE_NOT_SAVED);
        short ssOff = ssOffset(CLASS_MN_AAA, naiIdx);
        short ssN = ssLength(CLASS_MN_AAA, naiIdx);
        md.reset();
        md.update(chal, chalOff, (short) 1);          // highest-order byte
        md.update(ssPool, ssOff, ssN);
        md.update(mipRrqHash, (short) 0, HASH_LEN);
        md.doFinal(chal, chalOff, chalLen, out, outOff);
        st[ST_MIP_VALID] = 0;
    }
}
