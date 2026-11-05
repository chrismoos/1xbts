package org.onexbts.csim;

import javacard.framework.JCSystem;
import javacard.framework.Util;

/**
 * 3G / WLAN Authentication and Key Agreement (C.S0023-D §4.4.4), using the
 * S.S0055-A AKA functions f0-f5, f1* and f11 (in {@link Ss0055}). RAND and
 * AUTN in, f5 recovers the sequence number, f1 verifies MAC-A, and on a
 * fresh sequence number f2/f3/f4 produce RES, CK and IK (and f11 the UAK).
 * A stale sequence number returns AUTS built from f5* and f1*.
 *
 * The 3G and WLAN root keys K are provisioned by ADM. SQN_MS is the highest
 * accepted sequence number. A received SQN must exceed it (single-step
 * freshness, no window).
 */
public final class Aka {

    private static final short BLK = 16;
    public static final short MAC_LEN = 8;
    public static final short AK_LEN = 6;
    public static final short SQN_LEN = 6;
    public static final short AMF_LEN = 2;
    public static final short RES_LEN = 16;     // S.S0055 f2 output
    public static final short UAK_LEN = 16;

    public static final byte RESULT_OK = 0;
    public static final byte RESULT_MAC_FAIL = -1;
    public static final byte RESULT_SYNC = 1;

    private final Ss0055 kernel;
    private final byte[] k;        // 16, 3G root key
    private final byte[] wlanK;    // 16, WLAN root key
    private final byte[] sqnMs;    // 6, highest accepted SQN
    private boolean keyLoaded;
    private boolean wlanLoaded;

    private final byte[] ak;       // 6
    private final byte[] sqn;      // 6, derived from AUTN
    private final byte[] res;      // 16
    private final byte[] mac;      // 8, computed MAC-A / MAC-S
    private final byte[] ckik;     // 32, CK||IK from the last successful AKA
    private final byte[] uak;      // 16
    private final byte[] amf0;     // 2, resync AMF* (zero)
    private final byte[] okFlag;   // 1, keys present

    public Aka(Ss0055 ss0055) {
        kernel = ss0055;
        k = new byte[BLK];
        wlanK = new byte[BLK];
        sqnMs = new byte[SQN_LEN];
        ak = JCSystem.makeTransientByteArray(AK_LEN, JCSystem.CLEAR_ON_DESELECT);
        sqn = JCSystem.makeTransientByteArray(SQN_LEN, JCSystem.CLEAR_ON_DESELECT);
        res = JCSystem.makeTransientByteArray(RES_LEN, JCSystem.CLEAR_ON_DESELECT);
        mac = JCSystem.makeTransientByteArray(MAC_LEN, JCSystem.CLEAR_ON_DESELECT);
        ckik = JCSystem.makeTransientByteArray((short) 32, JCSystem.CLEAR_ON_DESELECT);
        uak = JCSystem.makeTransientByteArray(UAK_LEN, JCSystem.CLEAR_ON_DESELECT);
        amf0 = JCSystem.makeTransientByteArray(AMF_LEN, JCSystem.CLEAR_ON_DESELECT);
        okFlag = JCSystem.makeTransientByteArray((short) 1, JCSystem.CLEAR_ON_DESELECT);
    }

    public void loadRootKey(byte[] src, short off) {
        Util.arrayCopy(src, off, k, (short) 0, BLK);
        keyLoaded = true;
    }

    public void loadWlanKey(byte[] src, short off) {
        Util.arrayCopyNonAtomic(src, off, wlanK, (short) 0, BLK);
        wlanLoaded = true;
    }

    public boolean isKeyLoaded() {
        return keyLoaded;
    }

    public boolean isWlanKeyLoaded() {
        return wlanLoaded;
    }

    private static boolean sqnGreater(byte[] a, short ao, byte[] b, short bo) {
        for (short i = 0; i < SQN_LEN; i++) {
            short x = (short) (a[(short) (ao + i)] & 0xFF);
            short y = (short) (b[(short) (bo + i)] & 0xFF);
            if (x != y) return x > y;
        }
        return false;
    }

    /**
     * Run AKA over RAND(16) and AUTN(16) = SQN^AK || AMF || MAC. When wlan
     * is true the WLAN root key is used. On success writes 00 || CK(16) ||
     * IK(16) || RES_LEN(1) || RES(16). On a MAC failure it returns
     * RESULT_MAC_FAIL. A stale SQN writes 01 || AUTS(14).
     */
    public byte authenticate(boolean wlan, byte[] rand, short randOff, byte[] autn, short autnOff,
                             byte[] out, short outOff) {
        byte[] key = wlan ? wlanK : k;
        kernel.f5(key, (short) 0, rand, randOff, ak, (short) 0);
        for (short i = 0; i < SQN_LEN; i++) {
            sqn[i] = (byte) (autn[(short) (autnOff + i)] ^ ak[i]);
        }
        kernel.f1(key, (short) 0, rand, randOff, sqn, (short) 0,
                  autn, (short) (autnOff + SQN_LEN), mac, (short) 0);
        if (Util.arrayCompare(mac, (short) 0, autn, (short) (autnOff + SQN_LEN + AMF_LEN), MAC_LEN) != 0) {
            return RESULT_MAC_FAIL;
        }
        if (!sqnGreater(sqn, (short) 0, sqnMs, (short) 0)) {
            kernel.f5star(key, (short) 0, rand, randOff, ak, (short) 0);   // AK*
            short o = (short) (outOff + 1);
            for (short i = 0; i < SQN_LEN; i++) out[(short) (o + i)] = (byte) (sqnMs[i] ^ ak[i]);
            amf0[0] = 0;
            amf0[1] = 0;
            kernel.f1star(key, (short) 0, rand, randOff, sqnMs, (short) 0, amf0, (short) 0, mac, (short) 0);
            Util.arrayCopyNonAtomic(mac, (short) 0, out, (short) (o + SQN_LEN), MAC_LEN);
            out[outOff] = RESULT_SYNC;
            return RESULT_SYNC;
        }
        Util.arrayCopyNonAtomic(sqn, (short) 0, sqnMs, (short) 0, SQN_LEN);
        kernel.f2(key, (short) 0, rand, randOff, res, (short) 0);
        kernel.f3(key, (short) 0, rand, randOff, ckik, (short) 0);
        kernel.f4(key, (short) 0, rand, randOff, ckik, BLK);
        kernel.f11(key, (short) 0, rand, randOff, uak, (short) 0);
        okFlag[0] = 1;
        out[outOff] = RESULT_OK;
        Util.arrayCopyNonAtomic(ckik, (short) 0, out, (short) (outOff + 1), BLK);
        Util.arrayCopyNonAtomic(ckik, BLK, out, (short) (outOff + 1 + BLK), BLK);
        out[(short) (outOff + 1 + BLK + BLK)] = (byte) RES_LEN;
        Util.arrayCopyNonAtomic(res, (short) 0, out, (short) (outOff + 2 + BLK + BLK), RES_LEN);
        return RESULT_OK;
    }

    public short okResponseLen() {
        return (short) (1 + BLK + BLK + 1 + RES_LEN);     // 50
    }

    public short syncResponseLen() {
        return (short) (1 + SQN_LEN + MAC_LEN);           // 15
    }

    /** CK || IK from the most recent successful AKA, or false if none. */
    public boolean keysValid() {
        return okFlag[0] != 0;
    }

    public void copyKeys(byte[] out, short off) {
        Util.arrayCopyNonAtomic(ckik, (short) 0, out, off, (short) 32);
    }

    /** The UAK (f11) derived by the last successful AKA, for UMAC. */
    public void copyUak(byte[] out, short off) {
        Util.arrayCopyNonAtomic(uak, (short) 0, out, off, UAK_LEN);
    }

    /** The 3G root key K (RKP), used as the service-key derivation seed. */
    public void copyRootKeyK(byte[] out, short off) {
        Util.arrayCopyNonAtomic(k, (short) 0, out, off, BLK);
    }
}
