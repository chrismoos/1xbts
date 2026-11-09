package org.onexbts.csim;

import javacard.framework.JCSystem;
import javacard.framework.Util;

/**
 * CAVE authentication engine (S.S0053-0 v2.0 §2) and the CAVE-derived
 * procedures a CSIM performs: A-key storage and checksum (§2.2), SSD
 * generation and update (§2.3), authentication signatures (§2.4) and
 * CMEA-key / voice-privacy-mask generation (§2.5.1).
 *
 * CAVE is a table-driven non-linear mixing function: a 32-bit LFSR,
 * sixteen 8-bit mixing registers and a 256-entry lookup table, run for
 * four or eight rounds. No platform crypto provider is needed.
 *
 * All arithmetic stays in byte and short. The 18-bit authentication
 * signature is produced as three bytes rather than a 32-bit int so the
 * engine runs on a card without the optional int type. CaveTable is
 * split into two 128-byte halves: the JC 3.0.4 converter on the
 * sysmoISIM-SJA5 corrupts a static byte[] initializer past about 240
 * bytes (see FileSystem), so a single 256-byte table cannot be compiled
 * in.
 *
 * The A-key and active SSD are persistent. Candidate SSD is transient. The working registers and
 * the "saved registers" that hold a Run CAVE result for Generate Key/VPM
 * are RAM cleared on card reset, not on deselect: the saved registers
 * must survive the ME re-selecting the CSIM (C.S0023-D §4.4.4.2).
 */
public final class Cave {

    /** Authentication Algorithm Version, S.S0053 §1.1 (fixed 0xC7). */
    private static final byte AAV = (byte) 0xC7;

    private static final byte[] CAVE_TAB_LO = {
        (byte) 0xd9, (byte) 0x23, (byte) 0x5f, (byte) 0xe6, (byte) 0xca, (byte) 0x68, (byte) 0x97, (byte) 0xb0,
        (byte) 0x7b, (byte) 0xf2, (byte) 0x0c, (byte) 0x34, (byte) 0x11, (byte) 0xa5, (byte) 0x8d, (byte) 0x4e,
        (byte) 0x0a, (byte) 0x46, (byte) 0x77, (byte) 0x8d, (byte) 0x10, (byte) 0x9f, (byte) 0x5e, (byte) 0x62,
        (byte) 0xf1, (byte) 0x34, (byte) 0xec, (byte) 0xa5, (byte) 0xc9, (byte) 0xb3, (byte) 0xd8, (byte) 0x2b,
        (byte) 0x59, (byte) 0x47, (byte) 0xe3, (byte) 0xd2, (byte) 0xff, (byte) 0xae, (byte) 0x64, (byte) 0xca,
        (byte) 0x15, (byte) 0x8b, (byte) 0x7d, (byte) 0x38, (byte) 0x21, (byte) 0xbc, (byte) 0x96, (byte) 0x00,
        (byte) 0x49, (byte) 0x56, (byte) 0x23, (byte) 0x15, (byte) 0x97, (byte) 0xe4, (byte) 0xcb, (byte) 0x6f,
        (byte) 0xf2, (byte) 0x70, (byte) 0x3c, (byte) 0x88, (byte) 0xba, (byte) 0xd1, (byte) 0x0d, (byte) 0xae,
        (byte) 0xe2, (byte) 0x38, (byte) 0xba, (byte) 0x44, (byte) 0x9f, (byte) 0x83, (byte) 0x5d, (byte) 0x1c,
        (byte) 0xde, (byte) 0xab, (byte) 0xc7, (byte) 0x65, (byte) 0xf1, (byte) 0x76, (byte) 0x09, (byte) 0x20,
        (byte) 0x86, (byte) 0xbd, (byte) 0x0a, (byte) 0xf1, (byte) 0x3c, (byte) 0xa7, (byte) 0x29, (byte) 0x93,
        (byte) 0xcb, (byte) 0x45, (byte) 0x5f, (byte) 0xe8, (byte) 0x10, (byte) 0x74, (byte) 0x62, (byte) 0xde,
        (byte) 0xb8, (byte) 0x77, (byte) 0x80, (byte) 0xd1, (byte) 0x12, (byte) 0x26, (byte) 0xac, (byte) 0x6d,
        (byte) 0xe9, (byte) 0xcf, (byte) 0xf3, (byte) 0x54, (byte) 0x3a, (byte) 0x0b, (byte) 0x95, (byte) 0x4e,
        (byte) 0xb1, (byte) 0x30, (byte) 0xa4, (byte) 0x96, (byte) 0xf8, (byte) 0x57, (byte) 0x49, (byte) 0x8e,
        (byte) 0x05, (byte) 0x1f, (byte) 0x62, (byte) 0x7c, (byte) 0xc3, (byte) 0x2b, (byte) 0xda, (byte) 0xed
    };

    private static final byte[] CAVE_TAB_HI = {
        (byte) 0xbb, (byte) 0x86, (byte) 0x0d, (byte) 0x7a, (byte) 0x97, (byte) 0x13, (byte) 0x6c, (byte) 0x4e,
        (byte) 0x51, (byte) 0x30, (byte) 0xe5, (byte) 0xf2, (byte) 0x2f, (byte) 0xd8, (byte) 0xc4, (byte) 0xa9,
        (byte) 0x91, (byte) 0x76, (byte) 0xf0, (byte) 0x17, (byte) 0x43, (byte) 0x38, (byte) 0x29, (byte) 0x84,
        (byte) 0xa2, (byte) 0xdb, (byte) 0xef, (byte) 0x65, (byte) 0x5e, (byte) 0xca, (byte) 0x0d, (byte) 0xbc,
        (byte) 0xe7, (byte) 0xfa, (byte) 0xd8, (byte) 0x81, (byte) 0x6f, (byte) 0x00, (byte) 0x14, (byte) 0x42,
        (byte) 0x25, (byte) 0x7c, (byte) 0x5d, (byte) 0xc9, (byte) 0x9e, (byte) 0xb6, (byte) 0x33, (byte) 0xab,
        (byte) 0x5a, (byte) 0x6f, (byte) 0x9b, (byte) 0xd9, (byte) 0xfe, (byte) 0x71, (byte) 0x44, (byte) 0xc5,
        (byte) 0x37, (byte) 0xa2, (byte) 0x88, (byte) 0x2d, (byte) 0x00, (byte) 0xb6, (byte) 0x13, (byte) 0xec,
        (byte) 0x4e, (byte) 0x96, (byte) 0xa8, (byte) 0x5a, (byte) 0xb5, (byte) 0xd7, (byte) 0xc3, (byte) 0x8d,
        (byte) 0x3f, (byte) 0xf2, (byte) 0xec, (byte) 0x04, (byte) 0x60, (byte) 0x71, (byte) 0x1b, (byte) 0x29,
        (byte) 0x04, (byte) 0x79, (byte) 0xe3, (byte) 0xc7, (byte) 0x1b, (byte) 0x66, (byte) 0x81, (byte) 0x4a,
        (byte) 0x25, (byte) 0x9d, (byte) 0xdc, (byte) 0x5f, (byte) 0x3e, (byte) 0xb0, (byte) 0xf8, (byte) 0xa2,
        (byte) 0x91, (byte) 0x34, (byte) 0xf6, (byte) 0x5c, (byte) 0x67, (byte) 0x89, (byte) 0x73, (byte) 0x05,
        (byte) 0x22, (byte) 0xaa, (byte) 0xcb, (byte) 0xee, (byte) 0xbf, (byte) 0x18, (byte) 0xd0, (byte) 0x4d,
        (byte) 0xf5, (byte) 0x36, (byte) 0xae, (byte) 0x01, (byte) 0x2f, (byte) 0x94, (byte) 0xc3, (byte) 0x49,
        (byte) 0x8b, (byte) 0xbd, (byte) 0x58, (byte) 0x12, (byte) 0xe0, (byte) 0x77, (byte) 0x6c, (byte) 0xda
    };

    private static final short LO_HALF = 128;
    private static final short RETRY_LIMIT = 32;
    private static final short LFSR_HALF_MASK = 0x7FFF;
    private static final short LFSR_LOW_MASK = 0x00FF;
    private static final short LFSR_HIGH_MASK = (short) 0xFF00;

    // Persistent secret state (semi-permanent memory, S.S0053 §1.1).
    // A-key and SSD are semi-permanent, so these persist across power cycles.
    private boolean aKeyLoaded;
    private boolean ssdLoaded;

    private static final short FL_SAVED = 0;      // saved registers valid
    private static final short FL_RANDBS = 1;     // RANDBS issued
    private static final short FL_AUTHBS = 2;     // AUTHBS computed
    private static final short FL_CANDIDATE = 3;  // SSD candidate present
    private static final short FL_OTASP = 4;      // defer SSD commit to Commit
    private static final short FL_KEY = 5;

    // VPM octets valid for the saved registers. The ME may issue Generate
    // Key/VPM more than once per Run CAVE, so the result is computed once.
    private static final short VPM_DONE = 4;

    public static final short VPM_LEN = 65;
    // S.S0053 Exhibit 2-15: ten 6-octet VPM blocks, then a final 5-octet block.
    private static final short VPM_FULL_BLOCKS = 10;
    private static final short VPM_BLOCK_LEN = 6;
    public static final short KEY_LEN = 8;
    public static final short SIG_LEN = 3;

    private final byte[] state;
    private static final short SAVED_LFSR = 0;
    private static final short SAVED_RAND = 4;
    private static final short SAVED_DATA = 8;
    private static final short SAVED_ESN = 11;
    private static final short RANDBS = 15;
    private static final short AUTHBS = 19;
    private static final short DIV = 22;
    private static final short SSD_XOR_KEY = 25;
    private static final short FLAGS = 33;
    private static final short LFSR = 39;
    private static final short REG = 43;
    private static final short SHUFFLE = 59;
    private static final short ESN = 75;
    private static final short KEY = 79;
    private static final short VPM = 87;
    private static final short OFFSETS = 152;
    private static final short TABLE_READY = 157;
    private static final short TABLE = 158;
    private static final short TABLE_LEN = 2 * LO_HALF;
    private static final short SSD_A_NEW = TABLE + TABLE_LEN;
    private static final short SSD_B_NEW = SSD_A_NEW + 8;
    private static final short STATE_LEN = SSD_B_NEW + 8;

    private static final short BUILD_STATE = 1;
    private static final short BUILD_AKEY = 2;

    private final byte[] secrets;
    private static final short AKEY = 0;
    private static final short SSD_A = 8;
    private static final short SSD_B = 16;
    private static final short SECRET_LEN = 8;
    private static final short SECRETS_LEN = 24;

    public Cave(short[] progress) {
        progress[0] = BUILD_STATE;
        state = JCSystem.makeTransientByteArray(STATE_LEN, JCSystem.CLEAR_ON_RESET);
        progress[0] = BUILD_AKEY;
        secrets = new byte[SECRETS_LEN];
    }

    /**
     * SPASM signature AUTH_OTAPA (C.S0016-C §3.3.7): Auth_Signature with
     * SSD_AUTH = SSD_A XOR A_KEY, over RAND_OTAPA, AUTH_DATA (MIN1) and the
     * ESN. Writes the 3-byte (18-bit) result to out[outOff..+2].
     */
    public void authOtapa(byte[] rand, short randOff, byte[] data, short dataOff,
                          byte[] e, short esnOff, byte[] out, short outOff) {
        setEsn(e, esnOff);
        for (short i = 0; i < 8; i++) state[(short) (SSD_XOR_KEY + i)] = (byte) (secrets[(short) (SSD_A + i)] ^ secrets[(short) (AKEY + i)]);
        authSignature(rand, randOff, data, dataOff, state, SSD_XOR_KEY, false, out, outOff);
    }

    private short offset(short index) {
        return (short) (state[(short) (OFFSETS + index)] & 0xFF);
    }

    private void setOffset(short index, short value) {
        state[(short) (OFFSETS + index)] = (byte) value;
    }

    public short cmeaKeyOffset() { return KEY; }

    public short vpmOffset() { return VPM; }

    private void invalidateKeyVpm() {
        setFlag(FL_KEY, false);
        setOffset(VPM_DONE, (short) 0);
    }

    private boolean flag(short i) {
        return state[(short) (FLAGS + i)] != 0;
    }

    private void setFlag(short i, boolean v) {
        state[(short) (FLAGS + i)] = v ? (byte) 1 : (byte) 0;
    }

    // CAVE core (S.S0053 Exhibit 2-4)

    private void cave(short rounds) {
        // The RAM table is cleared on reset along with its validity byte.
        if (state[TABLE_READY] == 0) {
            Util.arrayCopyNonAtomic(CAVE_TAB_LO, (short) 0, state, TABLE, LO_HALF);
            Util.arrayCopyNonAtomic(CAVE_TAB_HI, (short) 0, state, (short) (TABLE + LO_HALF), LO_HALF);
            state[TABLE_READY] = 1;
        }
        final byte[] ram = state;
        short high = (short) (((ram[LFSR] & 0xFF) << 8) | (ram[(short) (LFSR + 1)] & 0xFF));
        short low = (short) (((ram[(short) (LFSR + 2)] & 0xFF) << 8) | (ram[(short) (LFSR + 3)] & 0xFF));
        short o0 = offset((short) 0);
        short o1 = offset((short) 1);
        short fb;
        for (short round = (short) (rounds - 1); round >= 0; round--) {
            byte reg0 = ram[(short) (REG + 0)];
            for (short r = 0; r < 16; r++) {
                short curU = (short) (ram[(short) (REG + r)] & 0xFF);
                short curLo = (short) (curU & 0x0F);
                short curHi = (short) (curU & 0xF0);
                short lowNibble;
                short hiNibble;
                short fail = 0;
                while (true) {
                    o0 = (short) ((o0 + ((high >> 8) ^ curU)) & 0xFF);
                    lowNibble = (short) (ram[(short) (TABLE + o0)] & 0x0F);
                    if (lowNibble != curLo) break;
                    fb = (short) (((high >> 6) ^ (low >> 2) ^ (low >> 1) ^ low) & 1);
                    low = (short) (((low >> 1) & LFSR_HALF_MASK) | (high << 15));
                    high = (short) (((high >> 1) & LFSR_HALF_MASK) | (fb << 15));
                    if (++fail == RETRY_LIMIT) { low = (short) ((low & LFSR_HIGH_MASK) | ((low + 1) & LFSR_LOW_MASK)); break; }
                }
                fail = 0;
                while (true) {
                    o1 = (short) ((o1 + (high ^ curU)) & 0xFF);
                    hiNibble = (short) (ram[(short) (TABLE + o1)] & 0xF0);
                    if (hiNibble != curHi) break;
                    fb = (short) (((high >> 6) ^ (low >> 2) ^ (low >> 1) ^ low) & 1);
                    low = (short) (((low >> 1) & LFSR_HALF_MASK) | (high << 15));
                    high = (short) (((high >> 1) & LFSR_HALF_MASK) | (fb << 15));
                    if (++fail == RETRY_LIMIT) { low = (short) ((low & LFSR_HIGH_MASK) | ((low + 1) & LFSR_LOW_MASK)); break; }
                }
                ram[(short) (REG + r)] = (byte) (((r == 15) ? reg0 : ram[(short) (REG + (short) (r + 1))]) ^ (lowNibble | hiNibble));
                fb = (short) (((high >> 6) ^ (low >> 2) ^ (low >> 1) ^ low) & 1);
                low = (short) (((low >> 1) & LFSR_HALF_MASK) | (high << 15));
                high = (short) (((high >> 1) & LFSR_HALF_MASK) | (fb << 15));
            }
            // Rotate the 128-bit register right by one bit and permute it
            // into shuffle in the same pass.
            short base = (short) (16 * round);
            short carry = (short) (ram[(short) (REG + 15)] & 1);
            for (short r = 0; r < 16; r++) {
                byte b = ram[(short) (REG + r)];
                ram[(short) (SHUFFLE + (short) (ram[(short) (TABLE + base + r)] & 0x0F))] =
                        (byte) (((b & 0xFF) >> 1) | (carry << 7));
                carry = (short) (b & 1);
            }
            Util.arrayCopyNonAtomic(state, SHUFFLE, state, REG, (short) 16);
        }
        ram[LFSR] = (byte) (high >> 8);
        ram[(short) (LFSR + 1)] = (byte) high;
        ram[(short) (LFSR + 2)] = (byte) (low >> 8);
        ram[(short) (LFSR + 3)] = (byte) low;
        setOffset((short) 0, o0);
        setOffset((short) 1, o1);
    }

    private static boolean lfsrZero(byte[] a, short off) {
        return (a[off] | a[(short) (off + 1)] | a[(short) (off + 2)] | a[(short) (off + 3)]) == 0;
    }

    // Secret state

    /** Load the 64-bit A-key (S.S0053 §2.2.2 writes A-key and zeros SSD). */
    public void loadAKey(byte[] src, short off) {
        // Atomic copies and plain stores, so OTASP Commit's transaction
        // covers them. The NonAtomic forms bypass an open transaction.
        Util.arrayCopy(src, off, secrets, AKEY, (short) 8);
        zeroSecret(SSD_A);
        zeroSecret(SSD_B);
        Util.arrayFillNonAtomic(state, SSD_A_NEW, (short) 16, (byte) 0);
        aKeyLoaded = true;
        ssdLoaded = false;
        setFlag(FL_CANDIDATE, false);
        invalidateKeyVpm();
    }

    private void zeroSecret(short offset) {
        for (short i = 0; i < SECRET_LEN; i++) secrets[(short) (offset + i)] = 0;
    }

    /** Load a pre-shared SSD_A || SSD_B directly (provisioning path). */
    public void loadSsd(byte[] src, short off) {
        Util.arrayCopyNonAtomic(src, off, secrets, SSD_A, (short) 8);
        Util.arrayCopyNonAtomic(src, (short) (off + 8), secrets, SSD_B, (short) 8);
        if (!ssdLoaded) ssdLoaded = true;
        setFlag(FL_CANDIDATE, false);
        invalidateKeyVpm();
    }

    public boolean isAKeyLoaded() {
        return aKeyLoaded;
    }

    /** Whether Run CAVE can produce a meaningful signature: either an
     *  A-key (SSD may still be zero / NeverShared) or a loaded SSD. */
    public boolean canAuthenticate() {
        return aKeyLoaded || ssdLoaded;
    }

    private void setEsn(byte[] e, short off) {
        Util.arrayCopyNonAtomic(e, off, state, ESN, (short) 4);
    }

    // Authentication signature (S.S0053 §2.4 Exhibit 2-14)

    /**
     * AUTH_SIGNATURE over RAND_CHALLENGE (4 bytes at randOff), AUTH_DATA
     * (3 bytes at dataOff), SSD_AUTH (8 bytes at ssdOff) and the ESN set
     * by the caller. Writes the 18-bit result to out[outOff..outOff+2],
     * most significant byte first. When save is true, freezes the
     * post-authentication registers for a later Generate Key/VPM.
     */
    private void authSignature(byte[] rand, short randOff, byte[] data, short dataOff,
                               byte[] ssdAuth, short ssdOff, boolean save,
                               byte[] out, short outOff) {
        for (short i = 0; i < 4; i++) {
            state[(short) (LFSR + i)] = (byte) (rand[(short) (randOff + i)]
                    ^ ssdAuth[(short) (ssdOff + i)]
                    ^ ssdAuth[(short) (ssdOff + i + 4)]);
        }
        if (lfsrZero(state, LFSR)) {
            Util.arrayCopyNonAtomic(rand, randOff, state, LFSR, (short) 4);
        }
        Util.arrayCopyNonAtomic(ssdAuth, ssdOff, state, REG, (short) 8);
        state[(short) (REG + 8)] = AAV;
        Util.arrayCopyNonAtomic(data, dataOff, state, (short) (REG + 9), (short) 3);
        Util.arrayCopyNonAtomic(state, ESN, state, (short) (REG + 12), (short) 4);
        setOffset((short) 0, (short) 128);
        setOffset((short) 1, (short) 128);
        cave((short) 8);
        out[outOff] = (byte) ((state[(short) (REG + 0)] ^ state[(short) (REG + 13)]) & 0x03);
        out[(short) (outOff + 1)] = (byte) (state[(short) (REG + 1)] ^ state[(short) (REG + 14)]);
        out[(short) (outOff + 2)] = (byte) (state[(short) (REG + 2)] ^ state[(short) (REG + 15)]);
        if (save) {
            setOffset((short) 2, offset((short) 0));
            setOffset((short) 3, offset((short) 1));
            Util.arrayCopyNonAtomic(state, LFSR, state, SAVED_LFSR, (short) 4);
            Util.arrayCopyNonAtomic(rand, randOff, state, SAVED_RAND, (short) 4);
            Util.arrayCopyNonAtomic(data, dataOff, state, SAVED_DATA, (short) 3);
            Util.arrayCopyNonAtomic(state, ESN, state, SAVED_ESN, (short) 4);
            setFlag(FL_SAVED, true);
            invalidateKeyVpm();
        }
    }

    /**
     * Run CAVE: compute AUTHR/AUTHU over the active SSD_A. rand holds the
     * 4-byte RAND_CHALLENGE, data the 3-byte AUTH_DATA, e the 4-byte ESN.
     * Writes the 3-byte signature to out[outOff].
     */
    public void runCave(byte[] rand, short randOff, byte[] data, short dataOff,
                        byte[] e, short esnOff, boolean save, byte[] out, short outOff) {
        setEsn(e, esnOff);
        authSignature(rand, randOff, data, dataOff, secrets, SSD_A, save, out, outOff);
    }

    // SSD generation and update (S.S0053 §2.3)

    /** SSD_Generation: build SSD_A_NEW / SSD_B_NEW from RANDSSD (7 bytes)
     *  and the ESN, held as a candidate until committed. */
    public void ssdGeneration(byte[] randssd, short rsOff, byte[] e, short esnOff) {
        setEsn(e, esnOff);
        for (short i = 0; i < 4; i++) {
            state[(short) (LFSR + i)] = (byte) (randssd[(short) (rsOff + i + 3)] ^ secrets[(short) (AKEY + i)] ^ secrets[(short) (AKEY + (short) (i + 4))]);
        }
        if (lfsrZero(state, LFSR)) {
            for (short i = 0; i < 4; i++) state[(short) (LFSR + i)] = randssd[(short) (rsOff + i + 3)];
        }
        Util.arrayCopyNonAtomic(secrets, AKEY, state, REG, (short) 8);
        state[(short) (REG + 8)] = AAV;
        Util.arrayCopyNonAtomic(randssd, rsOff, state, (short) (REG + 9), (short) 3);
        Util.arrayCopyNonAtomic(state, ESN, state, (short) (REG + 12), (short) 4);
        setOffset((short) 0, (short) 128);
        setOffset((short) 1, (short) 128);
        cave((short) 8);
        Util.arrayCopyNonAtomic(state, REG, state, SSD_A_NEW, (short) 8);
        Util.arrayCopyNonAtomic(state, (short) (REG + 8), state, SSD_B_NEW, (short) 8);
        setFlag(FL_CANDIDATE, true);
    }

    /** AUTHBS over the candidate SSD_A_NEW using the stored RANDBS and the
     *  given AUTH_DATA (MIN1). Result retained for Confirm SSD. */
    public void computeAuthBs(byte[] data, short dataOff) {
        authSignature(state, RANDBS, data, dataOff, state, SSD_A_NEW, false, state, AUTHBS);
        setFlag(FL_AUTHBS, true);
    }

    public boolean hasSsdCandidate() {
        return flag(FL_CANDIDATE);
    }

    public boolean hasRandBs() {
        return flag(FL_RANDBS);
    }

    public void setOtaspPending(boolean v) {
        setFlag(FL_OTASP, v);
    }

    public boolean isOtaspPending() {
        return flag(FL_OTASP);
    }

    /** Store a freshly generated RANDBS (handed to the ME in the Base
     *  Station Challenge response). */
    public void setRandBs(byte[] src, short off) {
        Util.arrayCopyNonAtomic(src, off, state, RANDBS, (short) 4);
        setFlag(FL_RANDBS, true);
        setFlag(FL_AUTHBS, false);
    }

    /** Confirm SSD: compare the ME-supplied AUTHBS against the computed
     *  value and, on a match, promote the candidate SSD. Returns true on
     *  a match. */
    public boolean confirmSsd(byte[] src, short off, boolean commitNow) {
        if (!flag(FL_AUTHBS)) return false;
        boolean ok = (src[off] == state[AUTHBS])
                && (src[(short) (off + 1)] == state[(short) (AUTHBS + 1)])
                && (src[(short) (off + 2)] == state[(short) (AUTHBS + 2)]);
        if (ok && commitNow) {
            persistCandidate(SSD_A_NEW, SSD_A);
            persistCandidate(SSD_B_NEW, SSD_B);
            if (!ssdLoaded) ssdLoaded = true;
            invalidateKeyVpm();
        }
        return ok;
    }

    private void persistCandidate(short source, short destination) {
        if (Util.arrayCompare(state, source, secrets, destination, SECRET_LEN) != 0) {
            Util.arrayCopy(state, source, secrets, destination, SECRET_LEN);
        }
    }

    /** Promote the candidate SSD unconditionally (OTASP Commit). */
    public void commitSsd() {
        if (!flag(FL_CANDIDATE)) return;
        persistCandidate(SSD_A_NEW, SSD_A);
        persistCandidate(SSD_B_NEW, SSD_B);
        if (!ssdLoaded) ssdLoaded = true;
        invalidateKeyVpm();
    }

    // CMEA key and VPM generation (S.S0053 §2.5.1 Exhibit 2-15)

    private void rollLfsr() {
        state[(short) (LFSR + 0)] = state[(short) (REG + 0)];
        state[(short) (LFSR + 1)] = state[(short) (REG + 1)];
        state[(short) (LFSR + 2)] = state[(short) (REG + 14)];
        state[(short) (LFSR + 3)] = state[(short) (REG + 15)];
        if (lfsrZero(state, LFSR)) {
            Util.arrayCopyNonAtomic(state, SAVED_RAND, state, LFSR, (short) 4);
        }
    }

    public boolean hasSavedRegisters() {
        return flag(FL_SAVED);
    }

    /** SSD_A || SSD_B (16 bytes), the key material for the Secure Mode
     *  ciphering key (C.S0016-C §3.3.8.1, KEY_IN_USE 0). */
    public void copySsdAB(byte[] out, short off) {
        Util.arrayCopyNonAtomic(secrets, SSD_A, out, off, (short) 8);
        Util.arrayCopyNonAtomic(secrets, SSD_B, out, (short) (off + 8), (short) 8);
    }

    /** Key_VPM_Generation: CMEA key (8 bytes) and, when vpmLast is not
     *  negative, the full 65-octet VPM, from the registers saved by the last
     *  Run CAVE with save on and SSD_B. Repeat calls reuse the result. */
    public void keyVpmGeneration(short vpmLast) {
        if (flag(FL_KEY) && offset(VPM_DONE) > vpmLast) return;
        for (short i = 0; i < 4; i++) {
            state[(short) (LFSR + i)] = (byte) (state[(short) (SAVED_LFSR + i)] ^ secrets[(short) (SSD_B + i)] ^ secrets[(short) (SSD_B + (short) (i + 4))]);
        }
        if (lfsrZero(state, LFSR)) {
            Util.arrayCopyNonAtomic(state, SAVED_RAND, state, LFSR, (short) 4);
        }
        Util.arrayCopyNonAtomic(secrets, SSD_B, state, REG, (short) 8);
        state[(short) (REG + 8)] = AAV;
        Util.arrayCopyNonAtomic(state, SAVED_DATA, state, (short) (REG + 9), (short) 3);
        Util.arrayCopyNonAtomic(state, SAVED_ESN, state, (short) (REG + 12), (short) 4);
        setOffset((short) 0, offset((short) 2));
        setOffset((short) 1, offset((short) 3));
        cave((short) 8);

        rollLfsr();
        cave((short) 4);
        for (short i = 0; i < 4; i++) state[(short) (KEY + i)] = (byte) (state[(short) (REG + (short) (i + 4))] ^ state[(short) (REG + (short) (i + 8))]);

        rollLfsr();
        cave((short) 4);
        for (short i = 4; i < 8; i++) state[(short) (KEY + i)] = (byte) (state[(short) (REG + i)] ^ state[(short) (REG + (short) (i + 4))]);
        setFlag(FL_KEY, true);
        setOffset(VPM_DONE, (short) 0);
        if (vpmLast < 0) return;

        short vp = 0;
        for (short it = 0; it < VPM_FULL_BLOCKS; it++) {
            rollLfsr();
            cave((short) 4);
            for (short r = 0; r < VPM_BLOCK_LEN; r++) {
                state[(short) (VPM + vp++)] = (byte) (state[(short) (REG + (short) (r + 2))] ^ state[(short) (REG + (short) (r + 8))]);
            }
        }
        rollLfsr();
        cave((short) 4);
        for (short j = 0; j < 5; j++) {
            state[(short) (VPM + vp++)] = (byte) (state[(short) (REG + (short) (j + 2))] ^ state[(short) (REG + (short) (j + 8))]);
        }
        setOffset(VPM_DONE, VPM_LEN);
    }

    public byte[] cmeaKey() {
        return state;
    }

    public byte[] vpm() {
        return state;
    }

    // A-key checksum (S.S0053 §2.2.1)

    /**
     * Compute the 6-decimal-digit A-key checksum over the stored A-key
     * and the supplied ESN and write it as ASCII to out[outOff..outOff+5].
     * The checksum is what a handset shows during manual A-key entry.
     */
    public void aKeyChecksum(byte[] e, short esnOff, byte[] out, short outOff) {
        setEsn(e, esnOff);
        if ((secrets[(short) (AKEY + 0)] | secrets[(short) (AKEY + 1)] | secrets[(short) (AKEY + 2)] | secrets[(short) (AKEY + 3)]) != 0) {
            Util.arrayCopyNonAtomic(secrets, AKEY, state, LFSR, (short) 4);
        } else {
            Util.arrayCopyNonAtomic(state, ESN, state, LFSR, (short) 4);
        }
        Util.arrayCopyNonAtomic(secrets, AKEY, state, REG, (short) 8);
        state[(short) (REG + 8)] = AAV;
        Util.arrayCopyNonAtomic(secrets, (short) (AKEY + 5), state, (short) (REG + 9), (short) 3);
        Util.arrayCopyNonAtomic(state, ESN, state, (short) (REG + 12), (short) 4);
        setOffset((short) 0, (short) 128);
        setOffset((short) 1, (short) 128);
        cave((short) 8);
        // 18-bit checksum as three bytes, then convert to six decimal digits.
        state[DIV] = (byte) ((state[(short) (REG + 0)] ^ state[(short) (REG + 13)]) & 0x03);
        state[(short) (DIV + 1)] = (byte) (state[(short) (REG + 1)] ^ state[(short) (REG + 14)]);
        state[(short) (DIV + 2)] = (byte) (state[(short) (REG + 2)] ^ state[(short) (REG + 15)]);
        for (short d = 5; d >= 0; d--) {
            short rem = 0;
            for (short i = 0; i < 3; i++) {
                short cur = (short) (((rem << 8) | (state[(short) (DIV + i)] & 0xFF)));
                state[(short) (DIV + i)] = (byte) (cur / 10);
                rem = (short) (cur % 10);
            }
            out[(short) (outOff + d)] = (byte) ('0' + rem);
        }
    }
}
