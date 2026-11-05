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
 * Secret state (A-key, SSD) is persistent. The per-authentication
 * working registers and the "saved registers" that hold a Run CAVE
 * result for Generate Key/VPM are transient and cleared on
 * deselect.
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

    // Persistent secret state (semi-permanent memory, S.S0053 §1.1).
    private final byte[] aKey;        // 8
    private final byte[] ssdA;        // 8, active
    private final byte[] ssdB;        // 8, active
    private final byte[] ssdANew;     // 8, candidate from SSD_Generation
    private final byte[] ssdBNew;     // 8, candidate from SSD_Generation
    // A-key and SSD are semi-permanent, so these persist across power cycles.
    private boolean aKeyLoaded;
    private boolean ssdLoaded;

    // Transient CAVE working state (cleared on deselect).
    private final byte[] lfsr;        // 4
    private final byte[] reg;         // 16 mixing registers
    private final byte[] shuffle;     // 16, permutation scratch for CAVE
    private final byte[] esn;         // 4, CAVE ESN of the current command
    private final short[] offsets;    // off1, off2, savedOff1, savedOff2

    // "Saved registers" that hold a Run CAVE result for Generate Key/VPM
    // (S.S0053 §2.4 SAVE_REGISTERS).
    private final byte[] savedLfsr;   // 4
    private final byte[] savedRand;   // 4
    private final byte[] savedData;   // 3 (AUTH_DATA)
    private final byte[] savedEsn;    // 4

    // Base Station Challenge / SSD update sequencing state.
    private final byte[] randbs;      // 4, last challenge handed to the ME
    private final byte[] authbs;      // 3, expected Confirm SSD value

    // Session-ephemeral validity flags, cleared on deselect together with
    // the transient arrays they guard. Indices below.
    private final byte[] fl;
    private static final short FL_SAVED = 0;      // saved registers valid
    private static final short FL_RANDBS = 1;     // RANDBS issued
    private static final short FL_AUTHBS = 2;     // AUTHBS computed
    private static final short FL_CANDIDATE = 3;  // SSD candidate present
    private static final short FL_OTASP = 4;      // defer SSD commit to Commit
    private static final short FL_COUNT = 5;

    // Outputs of Generate Key/VPM.
    private final byte[] cmeakey;     // 8
    private final byte[] vpm;         // 65 (VPM octets 0..0x40)

    public static final short VPM_LEN = 65;
    public static final short KEY_LEN = 8;
    public static final short SIG_LEN = 3;

    private final byte[] div;         // 3, A-key checksum decimal conversion
    private final byte[] ssdXorKey;   // 8, SSD_A ^ A_KEY for the SPASM signature

    public Cave() {
        aKey = new byte[(short) 8];
        ssdA = new byte[(short) 8];
        ssdB = new byte[(short) 8];
        ssdANew = new byte[(short) 8];
        ssdBNew = new byte[(short) 8];
        lfsr = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        reg = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        shuffle = JCSystem.makeTransientByteArray((short) 16, JCSystem.CLEAR_ON_DESELECT);
        esn = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        offsets = JCSystem.makeTransientShortArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        savedLfsr = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        savedRand = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        savedData = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        savedEsn = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        randbs = JCSystem.makeTransientByteArray((short) 4, JCSystem.CLEAR_ON_DESELECT);
        authbs = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        cmeakey = JCSystem.makeTransientByteArray(KEY_LEN, JCSystem.CLEAR_ON_DESELECT);
        vpm = JCSystem.makeTransientByteArray(VPM_LEN, JCSystem.CLEAR_ON_DESELECT);
        div = JCSystem.makeTransientByteArray((short) 3, JCSystem.CLEAR_ON_DESELECT);
        ssdXorKey = JCSystem.makeTransientByteArray((short) 8, JCSystem.CLEAR_ON_DESELECT);
        fl = JCSystem.makeTransientByteArray(FL_COUNT, JCSystem.CLEAR_ON_DESELECT);
    }

    /**
     * SPASM signature AUTH_OTAPA (C.S0016-C §3.3.7): Auth_Signature with
     * SSD_AUTH = SSD_A XOR A_KEY, over RAND_OTAPA, AUTH_DATA (MIN1) and the
     * ESN. Writes the 3-byte (18-bit) result to out[outOff..+2].
     */
    public void authOtapa(byte[] rand, short randOff, byte[] data, short dataOff,
                          byte[] e, short esnOff, byte[] out, short outOff) {
        setEsn(e, esnOff);
        for (short i = 0; i < 8; i++) ssdXorKey[i] = (byte) (ssdA[i] ^ aKey[i]);
        authSignature(rand, randOff, data, dataOff, ssdXorKey, (short) 0, false, out, outOff);
    }

    private boolean flag(short i) {
        return fl[i] != 0;
    }

    private void setFlag(short i, boolean v) {
        fl[i] = v ? (byte) 1 : (byte) 0;
    }

    // CAVE core (S.S0053 Exhibit 2-4)

    private byte tab(short idx) {
        return (idx < LO_HALF) ? CAVE_TAB_LO[idx] : CAVE_TAB_HI[(short) (idx - LO_HALF)];
    }

    private byte bitVal(byte octet, short bit) {
        return (byte) (((octet & 0xFF) << (7 - bit)) & 0x80);
    }

    private void lfsrCycle() {
        byte temp = (byte) (bitVal(lfsr[1], (short) 6)
                ^ bitVal(lfsr[3], (short) 2)
                ^ bitVal(lfsr[3], (short) 1)
                ^ bitVal(lfsr[3], (short) 0));
        for (short i = 3; i > 0; i--) {
            lfsr[i] = (byte) ((lfsr[i] & 0xFF) >> 1);
            if ((lfsr[(short) (i - 1)] & 0x01) != 0) lfsr[i] |= (byte) 0x80;
        }
        lfsr[0] = (byte) ((lfsr[0] & 0xFF) >> 1);
        lfsr[0] |= temp;
    }

    private void rotateRightRegisters() {
        byte last = reg[15];
        for (short i = 15; i > 0; i--) {
            reg[i] = (byte) ((reg[i] & 0xFF) >> 1);
            if ((reg[(short) (i - 1)] & 0x01) != 0) reg[i] |= (byte) 0x80;
        }
        reg[0] = (byte) ((reg[0] & 0xFF) >> 1);
        if ((last & 0x01) != 0) reg[0] |= (byte) 0x80;
    }

    private void cave(short rounds) {
        for (short round = (short) (rounds - 1); round >= 0; round--) {
            byte reg0 = reg[0];
            for (short r = 0; r < 16; r++) {
                byte lowNibble = 0;
                byte hiNibble = 0;
                short fail = 0;
                while (true) {
                    offsets[0] = (short) ((offsets[0] + ((lfsr[0] & 0xFF) ^ (reg[r] & 0xFF))) & 0xFF);
                    lowNibble = (byte) (tab(offsets[0]) & 0x0F);
                    if (lowNibble == (byte) (reg[r] & 0x0F)) {
                        lfsrCycle();
                        if (++fail == RETRY_LIMIT) { lfsr[3]++; break; }
                    } else {
                        break;
                    }
                }
                fail = 0;
                while (true) {
                    offsets[1] = (short) ((offsets[1] + ((lfsr[1] & 0xFF) ^ (reg[r] & 0xFF))) & 0xFF);
                    hiNibble = (byte) (tab(offsets[1]) & 0xF0);
                    if (hiNibble == (byte) (reg[r] & 0xF0)) {
                        lfsrCycle();
                        if (++fail == RETRY_LIMIT) { lfsr[3]++; break; }
                    } else {
                        break;
                    }
                }
                byte temp = (byte) (lowNibble | hiNibble);
                if (r == 15) reg[r] = (byte) (reg0 ^ temp);
                else reg[r] = (byte) (reg[(short) (r + 1)] ^ temp);
                lfsrCycle();
            }
            rotateRightRegisters();
            for (short r = 0; r < 16; r++) {
                byte slot = (byte) (tab((short) (16 * round + r)) & 0x0F);
                shuffle[slot] = reg[r];
            }
            Util.arrayCopyNonAtomic(shuffle, (short) 0, reg, (short) 0, (short) 16);
        }
    }

    private static boolean lfsrZero(byte[] a, short off) {
        return (a[off] | a[(short) (off + 1)] | a[(short) (off + 2)] | a[(short) (off + 3)]) == 0;
    }

    // Secret state

    /** Load the 64-bit A-key (S.S0053 §2.2.2 writes A-key and zeros SSD). */
    public void loadAKey(byte[] src, short off) {
        // Atomic copies and plain stores, so OTASP Commit's transaction
        // covers them. The NonAtomic forms bypass an open transaction.
        Util.arrayCopy(src, off, aKey, (short) 0, (short) 8);
        zero(ssdA);
        zero(ssdB);
        zero(ssdANew);
        zero(ssdBNew);
        aKeyLoaded = true;
        ssdLoaded = false;
        setFlag(FL_CANDIDATE, false);
    }

    private static void zero(byte[] a) {
        for (short i = 0; i < (short) a.length; i++) a[i] = 0;
    }

    /** Load a pre-shared SSD_A || SSD_B directly (provisioning path). */
    public void loadSsd(byte[] src, short off) {
        Util.arrayCopyNonAtomic(src, off, ssdA, (short) 0, (short) 8);
        Util.arrayCopyNonAtomic(src, (short) (off + 8), ssdB, (short) 0, (short) 8);
        ssdLoaded = true;
        setFlag(FL_CANDIDATE, false);
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
        Util.arrayCopyNonAtomic(e, off, esn, (short) 0, (short) 4);
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
            lfsr[i] = (byte) (rand[(short) (randOff + i)]
                    ^ ssdAuth[(short) (ssdOff + i)]
                    ^ ssdAuth[(short) (ssdOff + i + 4)]);
        }
        if (lfsrZero(lfsr, (short) 0)) {
            Util.arrayCopyNonAtomic(rand, randOff, lfsr, (short) 0, (short) 4);
        }
        Util.arrayCopyNonAtomic(ssdAuth, ssdOff, reg, (short) 0, (short) 8);
        reg[8] = AAV;
        Util.arrayCopyNonAtomic(data, dataOff, reg, (short) 9, (short) 3);
        Util.arrayCopyNonAtomic(esn, (short) 0, reg, (short) 12, (short) 4);
        offsets[0] = 128;
        offsets[1] = 128;
        cave((short) 8);
        out[outOff] = (byte) ((reg[0] ^ reg[13]) & 0x03);
        out[(short) (outOff + 1)] = (byte) (reg[1] ^ reg[14]);
        out[(short) (outOff + 2)] = (byte) (reg[2] ^ reg[15]);
        if (save) {
            offsets[2] = offsets[0];
            offsets[3] = offsets[1];
            Util.arrayCopyNonAtomic(lfsr, (short) 0, savedLfsr, (short) 0, (short) 4);
            Util.arrayCopyNonAtomic(rand, randOff, savedRand, (short) 0, (short) 4);
            Util.arrayCopyNonAtomic(data, dataOff, savedData, (short) 0, (short) 3);
            Util.arrayCopyNonAtomic(esn, (short) 0, savedEsn, (short) 0, (short) 4);
            setFlag(FL_SAVED, true);
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
        authSignature(rand, randOff, data, dataOff, ssdA, (short) 0, save, out, outOff);
    }

    // SSD generation and update (S.S0053 §2.3)

    /** SSD_Generation: build SSD_A_NEW / SSD_B_NEW from RANDSSD (7 bytes)
     *  and the ESN, held as a candidate until committed. */
    public void ssdGeneration(byte[] randssd, short rsOff, byte[] e, short esnOff) {
        setEsn(e, esnOff);
        for (short i = 0; i < 4; i++) {
            lfsr[i] = (byte) (randssd[(short) (rsOff + i + 3)] ^ aKey[i] ^ aKey[(short) (i + 4)]);
        }
        if (lfsrZero(lfsr, (short) 0)) {
            for (short i = 0; i < 4; i++) lfsr[i] = randssd[(short) (rsOff + i + 3)];
        }
        Util.arrayCopyNonAtomic(aKey, (short) 0, reg, (short) 0, (short) 8);
        reg[8] = AAV;
        Util.arrayCopyNonAtomic(randssd, rsOff, reg, (short) 9, (short) 3);
        Util.arrayCopyNonAtomic(esn, (short) 0, reg, (short) 12, (short) 4);
        offsets[0] = 128;
        offsets[1] = 128;
        cave((short) 8);
        Util.arrayCopyNonAtomic(reg, (short) 0, ssdANew, (short) 0, (short) 8);
        Util.arrayCopyNonAtomic(reg, (short) 8, ssdBNew, (short) 0, (short) 8);
        setFlag(FL_CANDIDATE, true);
    }

    /** AUTHBS over the candidate SSD_A_NEW using the stored RANDBS and the
     *  given AUTH_DATA (MIN1). Result retained for Confirm SSD. */
    public void computeAuthBs(byte[] data, short dataOff) {
        authSignature(randbs, (short) 0, data, dataOff, ssdANew, (short) 0, false, authbs, (short) 0);
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
        Util.arrayCopyNonAtomic(src, off, randbs, (short) 0, (short) 4);
        setFlag(FL_RANDBS, true);
        setFlag(FL_AUTHBS, false);
    }

    /** Confirm SSD: compare the ME-supplied AUTHBS against the computed
     *  value and, on a match, promote the candidate SSD. Returns true on
     *  a match. */
    public boolean confirmSsd(byte[] src, short off, boolean commitNow) {
        if (!flag(FL_AUTHBS)) return false;
        boolean ok = (src[off] == authbs[0])
                && (src[(short) (off + 1)] == authbs[1])
                && (src[(short) (off + 2)] == authbs[2]);
        if (ok && commitNow) {
            Util.arrayCopy(ssdANew, (short) 0, ssdA, (short) 0, (short) 8);
            Util.arrayCopy(ssdBNew, (short) 0, ssdB, (short) 0, (short) 8);
            ssdLoaded = true;
        }
        return ok;
    }

    /** Promote the candidate SSD unconditionally (OTASP Commit). */
    public void commitSsd() {
        if (!flag(FL_CANDIDATE)) return;
        Util.arrayCopy(ssdANew, (short) 0, ssdA, (short) 0, (short) 8);
        Util.arrayCopy(ssdBNew, (short) 0, ssdB, (short) 0, (short) 8);
        ssdLoaded = true;
    }

    // CMEA key and VPM generation (S.S0053 §2.5.1 Exhibit 2-15)

    private void rollLfsr() {
        lfsr[0] = reg[0];
        lfsr[1] = reg[1];
        lfsr[2] = reg[14];
        lfsr[3] = reg[15];
        if (lfsrZero(lfsr, (short) 0)) {
            Util.arrayCopyNonAtomic(savedRand, (short) 0, lfsr, (short) 0, (short) 4);
        }
    }

    public boolean hasSavedRegisters() {
        return flag(FL_SAVED);
    }

    /** SSD_A || SSD_B (16 bytes), the key material for the Secure Mode
     *  ciphering key (C.S0016-C §3.3.8.1, KEY_IN_USE 0). */
    public void copySsdAB(byte[] out, short off) {
        Util.arrayCopyNonAtomic(ssdA, (short) 0, out, off, (short) 8);
        Util.arrayCopyNonAtomic(ssdB, (short) 0, out, (short) (off + 8), (short) 8);
    }

    /** Key_VPM_Generation: CMEA key (8 bytes) and the 65-octet VPM, from
     *  the registers saved by the last Run CAVE with save on and SSD_B. */
    public void keyVpmGeneration() {
        for (short i = 0; i < 4; i++) {
            lfsr[i] = (byte) (savedLfsr[i] ^ ssdB[i] ^ ssdB[(short) (i + 4)]);
        }
        if (lfsrZero(lfsr, (short) 0)) {
            Util.arrayCopyNonAtomic(savedRand, (short) 0, lfsr, (short) 0, (short) 4);
        }
        Util.arrayCopyNonAtomic(ssdB, (short) 0, reg, (short) 0, (short) 8);
        reg[8] = AAV;
        Util.arrayCopyNonAtomic(savedData, (short) 0, reg, (short) 9, (short) 3);
        Util.arrayCopyNonAtomic(savedEsn, (short) 0, reg, (short) 12, (short) 4);
        offsets[0] = offsets[2];
        offsets[1] = offsets[3];
        cave((short) 8);

        rollLfsr();
        cave((short) 4);
        for (short i = 0; i < 4; i++) cmeakey[i] = (byte) (reg[(short) (i + 4)] ^ reg[(short) (i + 8)]);

        rollLfsr();
        cave((short) 4);
        for (short i = 4; i < 8; i++) cmeakey[i] = (byte) (reg[i] ^ reg[(short) (i + 4)]);

        short vp = 0;
        for (short it = 0; it < 10; it++) {
            rollLfsr();
            cave((short) 4);
            for (short r = 0; r < 6; r++) {
                vpm[vp++] = (byte) (reg[(short) (r + 2)] ^ reg[(short) (r + 8)]);
            }
        }
        rollLfsr();
        cave((short) 4);
        for (short j = 0; j < 5; j++) {
            vpm[vp++] = (byte) (reg[(short) (j + 2)] ^ reg[(short) (j + 8)]);
        }
    }

    public byte[] cmeaKey() {
        return cmeakey;
    }

    public byte[] vpm() {
        return vpm;
    }

    // A-key checksum (S.S0053 §2.2.1)

    /**
     * Compute the 6-decimal-digit A-key checksum over the stored A-key
     * and the supplied ESN and write it as ASCII to out[outOff..outOff+5].
     * The checksum is what a handset shows during manual A-key entry.
     */
    public void aKeyChecksum(byte[] e, short esnOff, byte[] out, short outOff) {
        setEsn(e, esnOff);
        if ((aKey[0] | aKey[1] | aKey[2] | aKey[3]) != 0) {
            Util.arrayCopyNonAtomic(aKey, (short) 0, lfsr, (short) 0, (short) 4);
        } else {
            Util.arrayCopyNonAtomic(esn, (short) 0, lfsr, (short) 0, (short) 4);
        }
        Util.arrayCopyNonAtomic(aKey, (short) 0, reg, (short) 0, (short) 8);
        reg[8] = AAV;
        Util.arrayCopyNonAtomic(aKey, (short) 5, reg, (short) 9, (short) 3);
        Util.arrayCopyNonAtomic(esn, (short) 0, reg, (short) 12, (short) 4);
        offsets[0] = 128;
        offsets[1] = 128;
        cave((short) 8);
        // 18-bit checksum as three bytes, then convert to six decimal digits.
        div[0] = (byte) ((reg[0] ^ reg[13]) & 0x03);
        div[1] = (byte) (reg[1] ^ reg[14]);
        div[2] = (byte) (reg[2] ^ reg[15]);
        for (short d = 5; d >= 0; d--) {
            short rem = 0;
            for (short i = 0; i < 3; i++) {
                short cur = (short) (((rem << 8) | (div[i] & 0xFF)));
                div[i] = (byte) (cur / 10);
                rem = (short) (cur % 10);
            }
            out[(short) (outOff + d)] = (byte) ('0' + rem);
        }
    }
}
