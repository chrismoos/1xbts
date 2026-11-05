package org.onexbts.csim;

import javacard.framework.ISOException;
import javacard.framework.JCSystem;
import javacardx.crypto.Cipher;
import javacard.security.AESKey;
import javacard.security.KeyBuilder;

/**
 * Int-free stub of the S.S0055-A SHA-based AKA kernel.
 *
 * The full kernel (f0-f5, f1 star, f5 star, f11, UMAC, service-key
 * derivation) runs a SHA-1 compression that needs the optional 32-bit int
 * type, and an int CAP has not been tested on the sysmoISIM-SJA5. The
 * SHA-based functions here return SW_FUNCTION_NOT_SUPPORTED, which disables
 * AKA, Secure Mode, Service Key Generation and the 3G-root-key OTASP path.
 * ESP (espSetKey/espMask) is byte- and AES-based and stays active. The full
 * implementation is ss0055/int/Ss0055.java, built with -Pss0055=int.
 */
public final class Ss0055 {

    private final byte[] espBlk;
    private final byte[] espOut;
    private final AESKey espKey;
    private final Cipher espAes;

    public Ss0055() {
        espBlk = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        espOut = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        espKey = (AESKey) KeyBuilder.buildKey(KeyBuilder.TYPE_AES, KeyBuilder.LENGTH_AES_128, false);
        espAes = Cipher.getInstance(Cipher.ALG_AES_BLOCK_128_ECB_NOPAD, false);
    }

    /** Schedule the ESP privacy key (SMCK). */
    public void espSetKey(byte[] key, short off) {
        espKey.setKey(key, off);
    }

    /**
     * XOR the ESP privacy mask into buf[off..off+len). The mask is a CTR
     * keystream: AES(key, fresh || counter) per 16-byte block, counter
     * big-endian in bytes (3 - i%4) of each column.
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

    private static void unsupported() {
        ISOException.throwIt(Const.SW_FUNCTION_NOT_SUPPORTED);
    }

    public void serviceItmRand(byte[] rkp, short rkpOff, byte[] out, short outOff) {
        unsupported();
    }

    public void serviceKey(byte[] itm, short itmOff, byte[] keyRand, short keyRandOff, short keyRandLen,
                           byte[] out, short outOff) {
        unsupported();
    }

    public void f0(byte[] seed, short seedOff, byte[] counter, short counterOff,
                   byte[] out, short outOff) {
        unsupported();
    }

    public void f1(byte[] k, short kOff, byte[] rand, short randOff,
                   byte[] sqn, short sqnOff, byte[] amf, short amfOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f1star(byte[] k, short kOff, byte[] rand, short randOff,
                       byte[] sqn, short sqnOff, byte[] amf, short amfOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f2(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f3(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f4(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f11(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f5(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void f5star(byte[] k, short kOff, byte[] rand, short randOff, byte[] out, short outOff) {
        unsupported();
    }

    public void umac(byte[] uak, short uakOff, byte[] mac, short macOff, short macLen, byte[] out, short outOff) {
        unsupported();
    }
}
