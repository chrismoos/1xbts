package org.onexbts.csim;

import javacard.framework.ISOException;
import javacard.framework.JCSystem;
import javacard.framework.Util;
import javacard.security.CryptoException;
import javacard.security.KeyBuilder;
import javacard.security.RSAPublicKey;
import javacard.security.RandomData;
import javacardx.crypto.Cipher;

/**
 * OTASP A-key Diffie-Hellman key exchange (C.S0016-C §5, C.S0023-D
 * §4.5.1 / §4.5.2), for A_KEY_P_REV '00000010'. The mobile picks a
 * secret exponent X and returns MS_RESULT = G^X mod P. Given the base
 * station's BS_RESULT it computes K = BS_RESULT^X mod P and keeps the
 * least significant 64 bits as the temporary A-key (C.S0016 §5.1.3),
 * committed into CAVE storage by the Commit command.
 *
 * Modular exponentiation runs on the card's RSA engine with ALG_RSA_NOPAD
 * (plain base^exp mod modulus). P is a 512-bit modulus. The private
 * exponent X is 160 bits (C.S0016 §5.1.1). A_KEY_P_REV 3/4/5 (which mix
 * in the AKA f0 kernel) are not implemented and are rejected with a
 * non-zero result code.
 */
public final class OtaspDh {

    public static final short MOD_LEN = 64;     // 512-bit P
    private static final short X_LEN = 20;      // 160-bit private exponent
    public static final short AKEY_LEN = 8;
    private static final byte A_KEY_P_REV_DH = 0x02;
    private static final byte A_KEY_P_REV_3G = 0x03;
    public static final byte RESULT_OK = 0x00;
    public static final byte RESULT_PREV_UNSUPPORTED = 0x05;
    public static final short RK_LEN = 16;

    private final byte[] modP;      // 64
    private final byte[] x;         // 20, secret exponent
    private final byte[] msResult;  // 64
    private final byte[] aKeyTemp;  // 8
    private final byte[] rkTemp;    // 16, 3G Root Key (p_rev 3)
    private final byte[] base;      // 64, RSA input block (transient)
    private final byte[] ctr;       // 8, f0 counter (transient)
    private final byte[] okFlag;    // transient: [0] MS_RESULT ready, [1] A-key temp ready, [2] RK ready
    private byte prev;              // A_KEY_P_REV of the current exchange

    // Built on first use, not in the constructor: a card without an RSA
    // engine (the SJA5 has none) must still install and run the CAVE path.
    private RSAPublicKey pub;
    private Cipher rsa;
    private RandomData rng;
    private boolean rsaTried;
    private final Ss0055 ss0055;

    public OtaspDh(Ss0055 kernel) {
        ss0055 = kernel;
        modP = new byte[MOD_LEN];
        x = new byte[X_LEN];
        msResult = new byte[MOD_LEN];
        aKeyTemp = new byte[AKEY_LEN];
        rkTemp = new byte[RK_LEN];
        base = JCSystem.makeTransientByteArray(MOD_LEN, JCSystem.CLEAR_ON_RESET);
        ctr = JCSystem.makeTransientByteArray((short) 8, JCSystem.CLEAR_ON_RESET);
        okFlag = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_RESET);
    }

    /**
     * Build the RSA modexp engine on first use. Returns false when the
     * card has no RSA, so OTASP A-key Diffie-Hellman cannot run here and
     * the caller reports it unsupported rather than faulting.
     */
    private boolean ensureRsa() {
        if (rsa != null) return true;
        if (rsaTried) return false;
        rsaTried = true;
        try {
            pub = (RSAPublicKey) KeyBuilder.buildKey(KeyBuilder.TYPE_RSA_PUBLIC,
                    KeyBuilder.LENGTH_RSA_512, false);
            rsa = Cipher.getInstance(Cipher.ALG_RSA_NOPAD, false);
            rng = RandomData.getInstance(RandomData.ALG_SECURE_RANDOM);
            return true;
        } catch (CryptoException e) {
            pub = null;
            rsa = null;
            return false;
        }
    }

    /** out = in^X mod P, all big-endian 64-byte blocks. */
    private void modExp(byte[] in, short inOff, byte[] out, short outOff) {
        pub.setModulus(modP, (short) 0, MOD_LEN);
        pub.setExponent(x, (short) 0, X_LEN);
        rsa.init(pub, Cipher.MODE_ENCRYPT);
        rsa.doFinal(in, inOff, MOD_LEN, out, outOff);
    }

    /** Left-pad a big-endian value into the 64-byte RSA input block. */
    private void loadBase(byte[] src, short off, short len) {
        Util.arrayFillNonAtomic(base, (short) 0, MOD_LEN, (byte) 0);
        if (len > MOD_LEN) len = MOD_LEN;
        Util.arrayCopyNonAtomic(src, off, base, (short) (MOD_LEN - len), len);
    }

    /**
     * MS Key Request (C.S0023-D §4.5.1): data RANDSeed(20), A_KEY_P_REV(1),
     * P_len(1), G_len(1), P, G. Generates X and MS_RESULT. Returns the
     * result code.
     */
    public byte msKeyRequest(byte[] data, short off, short len) {
        if (!ensureRsa()) return RESULT_PREV_UNSUPPORTED;
        prev = data[(short) (off + 20)];
        if (prev != A_KEY_P_REV_DH && prev != A_KEY_P_REV_3G) return RESULT_PREV_UNSUPPORTED;
        short pLen = (short) (data[(short) (off + 21)] & 0xFF);
        short gLen = (short) (data[(short) (off + 22)] & 0xFF);
        if (pLen == 0 || pLen > MOD_LEN) return RESULT_PREV_UNSUPPORTED;
        short pOff = (short) (off + 23);
        short gOff = (short) (pOff + pLen);
        Util.arrayFillNonAtomic(modP, (short) 0, MOD_LEN, (byte) 0);
        Util.arrayCopyNonAtomic(data, pOff, modP, (short) (MOD_LEN - pLen), pLen);
        // X: 160-bit secret, not less than 4.
        rng.generateData(x, (short) 0, X_LEN);
        if ((x[(short) (X_LEN - 1)] & 0xFF) < 4) x[(short) (X_LEN - 1)] |= 0x04;
        loadBase(data, gOff, gLen);
        modExp(base, (short) 0, msResult, (short) 0);
        okFlag[0] = 1;
        okFlag[1] = 0;
        return RESULT_OK;
    }

    /**
     * Key Generation Request (C.S0023-D §4.5.2): data BS_Result_Length(1),
     * BS_Result. Computes the A-key temp and writes Result Code(1),
     * MS_Result_Length(1), MS_Result(64) to out. Returns the response
     * length.
     */
    public short keyGenRequest(byte[] data, short off, short len, byte[] out, short outOff) {
        if (!ensureRsa() || okFlag[0] == 0) {
            out[outOff] = RESULT_PREV_UNSUPPORTED;
            out[(short) (outOff + 1)] = 0;
            return (short) 2;
        }
        short bsLen = (short) (data[off] & 0xFF);
        loadBase(data, (short) (off + 1), bsLen);
        modExp(base, (short) 0, base, (short) 0);   // base = BS_RESULT^X mod P
        if (prev == A_KEY_P_REV_3G) {
            // RAND_SEED = least significant 128 bits of the DH result, then
            // A_KEY (f0 counter 0) and RK (f0 counters 1, 2), C.S0016 §5.3.3.
            short seedOff = (short) (MOD_LEN - 16);
            Util.arrayFillNonAtomic(ctr, (short) 0, (short) 8, (byte) 0);
            ss0055.f0(base, seedOff, ctr, (short) 0, aKeyTemp, (short) 0);
            ctr[7] = 1;
            ss0055.f0(base, seedOff, ctr, (short) 0, rkTemp, (short) 0);
            ctr[7] = 2;
            ss0055.f0(base, seedOff, ctr, (short) 0, rkTemp, (short) 8);
            okFlag[2] = 1;
        } else {
            Util.arrayCopyNonAtomic(base, (short) (MOD_LEN - AKEY_LEN), aKeyTemp, (short) 0, AKEY_LEN);
            okFlag[2] = 0;
        }
        okFlag[1] = 1;
        out[outOff] = RESULT_OK;
        out[(short) (outOff + 1)] = (byte) MOD_LEN;
        Util.arrayCopyNonAtomic(msResult, (short) 0, out, (short) (outOff + 2), MOD_LEN);
        return (short) (2 + MOD_LEN);
    }

    public boolean hasAKeyTemp() {
        return okFlag[1] != 0;
    }

    public void copyAKeyTemp(byte[] out, short off) {
        Util.arrayCopyNonAtomic(aKeyTemp, (short) 0, out, off, AKEY_LEN);
    }

    /** True when the last exchange derived a 3G Root Key (p_rev 3). */
    public boolean hasRootKey() {
        return okFlag[2] != 0;
    }

    public void copyRootKey(byte[] out, short off) {
        Util.arrayCopyNonAtomic(rkTemp, (short) 0, out, off, RK_LEN);
    }
}
