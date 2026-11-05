package org.onexbts.csim;

import javacard.framework.JCSystem;
import javacard.framework.Util;
import javacard.security.AESKey;
import javacard.security.KeyBuilder;
import javacardx.crypto.Cipher;

/**
 * S.S0055-A SHA-based AKA kernel (§2.2.2), used by OTASP for the p_rev-3
 * A-key / 3G Root Key derivation (f0) and by Secure Mode for the ciphering
 * key SMCK (f3). Each function runs one SHA-1 compression from an IV
 * tweaked with the key, over a 512-bit payload holding the inputs, then
 * the whitening polynomial (A*X + B) mod G over GF(2^160).
 *
 * The SHA-1 compression needs 32-bit arithmetic, so this class uses the
 * optional int type.
 * The 80-round message schedule lives in a transient byte array to avoid
 * persistent writes. Validated against S.S0055-A Exhibit 4-3 vectors.
 */
public final class Ss0055 {

    private static final byte[] SHA_IV = {
        (byte) 0x67, (byte) 0x45, (byte) 0x23, (byte) 0x01,
        (byte) 0xef, (byte) 0xcd, (byte) 0xab, (byte) 0x89,
        (byte) 0x98, (byte) 0xba, (byte) 0xdc, (byte) 0xfe,
        (byte) 0x10, (byte) 0x32, (byte) 0x54, (byte) 0x76,
        (byte) 0xc3, (byte) 0xd2, (byte) 0xe1, (byte) 0xf0
    };
    private static final byte[] POLY_A = {
        (byte) 0x9d, (byte) 0xe9, (byte) 0xc9, (byte) 0xc8, (byte) 0xef, (byte) 0xd5, (byte) 0x78, (byte) 0x11,
        (byte) 0x48, (byte) 0x23, (byte) 0x14, (byte) 0x01, (byte) 0x90, (byte) 0x1f, (byte) 0x2d, (byte) 0x49,
        (byte) 0x3f, (byte) 0x4c, (byte) 0x63, (byte) 0x65
    };
    private static final byte[] POLY_B = {
        (byte) 0x75, (byte) 0xef, (byte) 0xd1, (byte) 0x5c, (byte) 0x4b, (byte) 0x8f, (byte) 0x8f, (byte) 0x51,
        (byte) 0x4e, (byte) 0xf3, (byte) 0xbc, (byte) 0xc3, (byte) 0x79, (byte) 0x4a, (byte) 0x76, (byte) 0x5e,
        (byte) 0x7e, (byte) 0xec, (byte) 0x45, (byte) 0xe0
    };
    private static final byte[] POLY_G = {
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, (byte) 0x2d
    };
    private static final byte[] FMK = { (byte) 0x41, (byte) 0x48, (byte) 0x41, (byte) 0x47 };

    private static final byte FI_F0 = (byte) 0x41;
    private static final byte FI_F1 = (byte) 0x42;
    private static final byte FI_F3 = (byte) 0x45;

    private final byte[] digest;   // 20, SHA state (= whiten input/output)
    private final byte[] buf;      // 64, SHA payload
    private final byte[] w;        // 64, SHA message schedule (16 words)
    private final byte[] z;        // 40, whiten accumulator
    private final byte[] q;        // 21, modred scratch
    private final byte[] espBlk;   // 16, ESP keystream block
    private final byte[] espOut;   // 16, ESP keystream output
    private final AESKey espKey;
    private final Cipher espAes;

    public Ss0055() {
        digest = JCSystem.makeTransientByteArray((short) 20, JCSystem.CLEAR_ON_DESELECT);
        buf = JCSystem.makeTransientByteArray((short) 64, JCSystem.CLEAR_ON_DESELECT);
        w = JCSystem.makeTransientByteArray((short) 64, JCSystem.CLEAR_ON_DESELECT);
        z = JCSystem.makeTransientByteArray((short) 40, JCSystem.CLEAR_ON_DESELECT);
        q = JCSystem.makeTransientByteArray((short) 21, JCSystem.CLEAR_ON_DESELECT);
        espBlk = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        espOut = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        espKey = (AESKey) KeyBuilder.buildKey(KeyBuilder.TYPE_AES, KeyBuilder.LENGTH_AES_128, false);
        espAes = Cipher.getInstance(Cipher.ALG_AES_BLOCK_128_ECB_NOPAD, false);
    }

    // Enhanced Subscriber Privacy (S.S0055-A §2.3.2.2), byte-aligned

    /** Schedule the ESP privacy key (SMCK). */
    public void espSetKey(byte[] key, short off) {
        espKey.setKey(key, off);
    }

    /**
     * XOR the ESP privacy mask into buf[off..off+len). The mask is a CTR
     * keystream: AES(key, fresh || counter) per 16-byte block, counter
     * big-endian in bytes (3 - i%4) of each column. Byte-aligned form
     * (bit_offset 0), sufficient for OTASP PARAM_DATA.
     */
    public void espMask(byte[] fresh, short freshOff, short freshLen, byte[] buf, short off, short len) {
        short done = 0;
        short counter = 0;
        while (done < len) {
            for (short i = 0; i < 16; i++) {
                if (i < freshLen) {
                    espBlk[i] = fresh[(short) (freshOff + i)];
                } else {
                    short k = (short) (3 - (i & 3));
                    espBlk[i] = (byte) ((counter >> (k * 8)) & 0xFF);
                }
            }
            espAes.init(espKey, Cipher.MODE_ENCRYPT);
            espAes.doFinal(espBlk, (short) 0, (short) 16, espOut, (short) 0);
            short n = (short) (len - done);
            if (n > 16) n = 16;
            for (short i = 0; i < n; i++) {
                buf[(short) (off + done + i)] ^= espOut[i];
            }
            done += n;
            counter++;
        }
    }

    // SHA-1 single-block compression (custom IV in digest, message in buf)

    private static int wd(byte[] a, short off) {
        return ((a[off] & 0xFF) << 24) | ((a[(short) (off + 1)] & 0xFF) << 16)
             | ((a[(short) (off + 2)] & 0xFF) << 8) | (a[(short) (off + 3)] & 0xFF);
    }

    private static void stw(byte[] a, short off, int v) {
        a[off] = (byte) (v >>> 24);
        a[(short) (off + 1)] = (byte) (v >>> 16);
        a[(short) (off + 2)] = (byte) (v >>> 8);
        a[(short) (off + 3)] = (byte) v;
    }

    private void shaHash() {
        int a = wd(digest, (short) 0);
        int b = wd(digest, (short) 4);
        int c = wd(digest, (short) 8);
        int d = wd(digest, (short) 12);
        int e = wd(digest, (short) 16);
        int a0 = a, b0 = b, c0 = c, d0 = d, e0 = e;
        for (short i = 0; i < 16; i++) stw(w, (short) (i << 2), wd(buf, (short) (i << 2)));
        for (short i = 0; i < 80; i++) {
            short s = (short) ((i & 0x0f) << 2);
            if (i >= 16) {
                int t = wd(w, (short) (((i - 3) & 0x0f) << 2)) ^ wd(w, (short) (((i - 8) & 0x0f) << 2))
                      ^ wd(w, (short) (((i - 14) & 0x0f) << 2)) ^ wd(w, s);
                stw(w, s, (t << 1) | (t >>> 31));
            }
            int ft;
            if (i < 20) ft = ((b & c) | (~b & d)) + 0x5a827999;
            else if (i < 40) ft = (b ^ c ^ d) + 0x6ed9eba1;
            else if (i < 60) ft = ((b & c) | (b & d) | (c & d)) + 0x8f1bbcdc;
            else ft = (b ^ c ^ d) + 0xca62c1d6;
            int t2 = ((a << 5) | (a >>> 27)) + ft + e + wd(w, s);
            e = d;
            d = c;
            c = (b << 30) | (b >>> 2);
            b = a;
            a = t2;
        }
        stw(digest, (short) 0, a + a0);
        stw(digest, (short) 4, b + b0);
        stw(digest, (short) 8, c + c0);
        stw(digest, (short) 12, d + d0);
        stw(digest, (short) 16, e + e0);
    }

    // whiten: (A*X + B) mod G over GF(2^160) (S.S0055-A Exhibit 3-9)

    private void modred(short shift, byte[] base) {
        for (short i = 0; i < 20; i++) q[i] = base[i];
        q[20] = 0;
        short byteshift = (short) (shift / 8);
        short bitshift = (short) (shift % 8);
        if (bitshift != 0) {
            short yn = 0;
            for (short i = 0; i <= 20; i++) {
                short yn1 = yn;
                yn = (short) (q[i] & 0xFF);
                q[i] = (byte) (((q[i] & 0xFF) >> (8 - bitshift)) | (yn1 << bitshift));
            }
            byteshift++;
        }
        for (short i = 0; i < 20; i++) {
            z[(short) (i + 20 - byteshift)] ^= q[i];
        }
        if (bitshift != 0) {
            z[(short) (40 - byteshift)] ^= q[20];
        }
    }

    private void whiten() {
        Util.arrayFillNonAtomic(z, (short) 0, (short) 40, (byte) 0);
        for (short i = 0; i < 20; i++) {
            for (short j = 0; j < 8; j++) {
                if ((((digest[i] & 0xFF) << j) & 0x80) != 0) {
                    modred((short) (159 - (i * 8 + j)), POLY_A);
                }
            }
        }
        for (short i = 0; i < 20; i++) {
            for (short j = 0; j < 8; j++) {
                if ((((z[i] & 0xFF) << j) & 0x80) != 0) {
                    modred((short) (159 - (i * 8 + j)), POLY_G);
                }
            }
        }
        for (short i = 0; i < 20; i++) digest[i] = (byte) (z[(short) (i + 20)] ^ POLY_B[i]);
    }

    // Service-key family key 0x534B474B (C.S0016-C §3.3.10).
    private static final byte[] SKGK = { (byte) 0x53, (byte) 0x4b, (byte) 0x47, (byte) 0x4b };

    private void initKey(byte[] key, short keyOff, short keyLen) {
        initKeyFmk(key, keyOff, keyLen, FMK);
    }

    private void initKeyFmk(byte[] key, short keyOff, short keyLen, byte[] fmk) {
        Util.arrayCopyNonAtomic(SHA_IV, (short) 0, digest, (short) 0, (short) 20);
        for (short i = 0; i < keyLen; i++) {
            digest[i] ^= key[(short) (keyOff + i)];
        }
        Util.arrayFillNonAtomic(buf, (short) 0, (short) 64, (byte) 0x5c);
        for (short i = 0; i < 4; i++) buf[(short) (12 + i)] ^= fmk[i];
    }

    /** Service-key intermediate value ITM_RAND = f0(RKP, fi=f0, Fmk=SKGK)
     *  twice (C.S0016-C §3.3.10.1), 128 bits. */
    public void serviceItmRand(byte[] rkp, short rkpOff, byte[] out, short outOff) {
        for (byte j = 0; j < 2; j++) {
            initKeyFmk(rkp, rkpOff, (short) 16, SKGK);
            buf[11] ^= FI_F0;
            buf[7] ^= j;      // counter in bytes 0..7 / 16..23 / 32..39 / 48..55
            buf[23] ^= j;
            buf[39] ^= j;
            buf[55] ^= j;
            shaHash();
            whiten();
            Util.arrayCopyNonAtomic(digest, (short) 0, out, (short) (outOff + 8 * j), (short) 8);
        }
    }

    /** Service root key = f3(ITM_RAND, RAND=keyRand padded, Fmk=SKGK)
     *  (C.S0016-C §3.3.10.2), 128 bits. */
    public void serviceKey(byte[] itm, short itmOff, byte[] keyRand, short keyRandOff, short keyRandLen,
                           byte[] out, short outOff) {
        for (byte j = 0; j < 2; j++) {
            initKeyFmk(itm, itmOff, (short) 16, SKGK);
            buf[11] ^= FI_F3;
            for (short i = 0; i < keyRandLen && i < 16; i++) {
                buf[(short) (24 + i)] ^= keyRand[(short) (keyRandOff + i)];
            }
            buf[3] ^= j;
            buf[19] ^= j;
            buf[35] ^= j;
            buf[51] ^= j;
            shaHash();
            whiten();
            Util.arrayCopyNonAtomic(digest, (short) 0, out, (short) (outOff + 8 * j), (short) 8);
        }
    }

    // AKA functions f0-f5, f1*, f5*, f11 (S.S0055-A §2.2.2)

    private static final byte FI_F2 = (byte) 0x44;
    private static final byte FI_F4 = (byte) 0x46;
    private static final byte FI_F5 = (byte) 0x47;
    private static final byte FI_F1STAR = (byte) 0x43;
    private static final byte FI_F5STAR = (byte) 0x48;
    private static final byte FI_F11 = (byte) 0x50;

    /** f0 (§2.2.2.2): 64 pseudo-random bits from a 128-bit seed and an
     *  8-byte counter, into out[outOff..+7]. */
    public void f0(byte[] seed, short seedOff, byte[] counter, short counterOff,
                   byte[] out, short outOff) {
        initKey(seed, seedOff, (short) 16);
        buf[11] ^= FI_F0;
        for (short i = 0; i < 8; i++) {
            byte cv = counter[(short) (counterOff + i)];
            buf[i] ^= cv;
            buf[(short) (i + 16)] ^= cv;
            buf[(short) (i + 32)] ^= cv;
            buf[(short) (i + 48)] ^= cv;
        }
        shaHash();
        whiten();
        Util.arrayCopyNonAtomic(digest, (short) 0, out, outOff, (short) 8);
    }

    /** f1 / f1* (§2.2.2.3-4): MAC-A / MAC-S over K, RAND, SQN, AMF. 8 bytes.
     *  RAND at buf[16..31], SQN at buf[34..39], AMF at buf[42..43]. */
    private void f1gen(byte fi, byte[] k, short kOff, byte[] rand, short randOff,
                       byte[] sqn, short sqnOff, byte[] amf, short amfOff,
                       byte[] out, short outOff) {
        initKey(k, kOff, (short) 16);
        buf[11] ^= fi;
        for (short i = 0; i < 16; i++) buf[(short) (16 + i)] ^= rand[(short) (randOff + i)];
        for (short i = 0; i < 6; i++) buf[(short) (34 + i)] ^= sqn[(short) (sqnOff + i)];
        for (short i = 0; i < 2; i++) buf[(short) (42 + i)] ^= amf[(short) (amfOff + i)];
        shaHash();
        whiten();
        Util.arrayCopyNonAtomic(digest, (short) 0, out, outOff, (short) 8);
    }

    public void f1(byte[] k, short kOff, byte[] rand, short randOff,
                   byte[] sqn, short sqnOff, byte[] amf, short amfOff, byte[] out, short outOff) {
        f1gen(FI_F1, k, kOff, rand, randOff, sqn, sqnOff, amf, amfOff, out, outOff);
    }

    public void f1star(byte[] k, short kOff, byte[] rand, short randOff,
                       byte[] sqn, short sqnOff, byte[] amf, short amfOff, byte[] out, short outOff) {
        f1gen(FI_F1STAR, k, kOff, rand, randOff, sqn, sqnOff, amf, amfOff, out, outOff);
    }

    /** f2 / f3 / f4 / f11 (§2.2.2.5-7, 10): 128-bit output over K and RAND,
     *  two SHA iterations. RAND at buf[24..39]. */
    private void multi(byte fi, byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        for (byte j = 0; j < 2; j++) {
            initKey(k, kOff, (short) 16);
            buf[11] ^= fi;
            for (short i = 0; i < 16; i++) buf[(short) (24 + i)] ^= rand[(short) (randOff + i)];
            buf[3] ^= j;
            buf[19] ^= j;
            buf[35] ^= j;
            buf[51] ^= j;
            shaHash();
            whiten();
            Util.arrayCopyNonAtomic(digest, (short) 0, out, (short) (outOff + 8 * j), (short) 8);
        }
    }

    public void f2(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        multi(FI_F2, k, kOff, rand, randOff, out, outOff);
    }

    public void f3(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        multi(FI_F3, k, kOff, rand, randOff, out, outOff);
    }

    public void f4(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        multi(FI_F4, k, kOff, rand, randOff, out, outOff);
    }

    public void f11(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        multi(FI_F11, k, kOff, rand, randOff, out, outOff);
    }

    /** f5 / f5* (§2.2.2.8-9): 48-bit anonymity key over K and RAND, one SHA
     *  iteration. RAND at buf[16..31]. */
    private void f5gen(byte fi, byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        initKey(k, kOff, (short) 16);
        buf[11] ^= fi;
        for (short i = 0; i < 16; i++) buf[(short) (16 + i)] ^= rand[(short) (randOff + i)];
        shaHash();
        whiten();
        Util.arrayCopyNonAtomic(digest, (short) 0, out, outOff, (short) 6);
    }

    public void f5(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        f5gen(FI_F5, k, kOff, rand, randOff, out, outOff);
    }

    public void f5star(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        f5gen(FI_F5STAR, k, kOff, rand, randOff, out, outOff);
    }

    /**
     * UMAC (S.S0078-A §2.1.2.2): one SHA-1 compression keyed by UAK XOR the
     * SHA-1 IV, over MAC || 0x80 padding, returning the most significant
     * macLen bytes. No whitening.
     */
    public void umac(byte[] uak, short uakOff, byte[] mac, short macOff, short macLen, byte[] out, short outOff) {
        Util.arrayCopyNonAtomic(SHA_IV, (short) 0, digest, (short) 0, (short) 20);
        for (short i = 0; i < 16; i++) digest[i] ^= uak[(short) (uakOff + i)];
        Util.arrayFillNonAtomic(buf, (short) 0, (short) 64, (byte) 0);
        Util.arrayCopyNonAtomic(mac, macOff, buf, (short) 0, macLen);
        buf[macLen] = (byte) 0x80;
        shaHash();
        Util.arrayCopyNonAtomic(digest, (short) 0, out, outOff, macLen);
    }
}
