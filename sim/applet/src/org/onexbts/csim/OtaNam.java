package org.onexbts.csim;

import javacard.framework.Util;

/**
 * OTASP NAM parameter-block codecs (C.S0016-C §4.5.2): decode a
 * downloaded block into the EFs that make up the NAM, and encode a block
 * back from the EFs for a Configuration Request. Blocks are bit-packed,
 * most significant bit first.
 *
 * Implemented blocks: Mobile Directory Number ('01') to EF_MDN, and CDMA
 * NAM Download ('02') to EF_IMSI_M and EF_ACCOLC (its SID/NID list maps
 * to EF_CDMAHOME, left to a per-record decoder). The EF byte layouts
 * follow C.S0065-B §5.2.2 (IMSI) and C.S0023-D §3.4.34 (MDN).
 */
public final class OtaNam {

    public static final byte BLOCK_CDMA_ANALOG_NAM = 0x00;
    public static final byte BLOCK_MDN = 0x01;
    public static final byte BLOCK_CDMA_NAM = 0x02;
    public static final byte BLOCK_IMSI_T = 0x03;

    // Parameter-block field widths in bits (C.S0016-C §4.5.2).
    private static final short N_DIGITS_BITS = 4;
    private static final short DIGIT_BITS = 4;
    private static final short FIRSTCHP_BITS = 11;
    private static final short HOME_SID_BITS = 15;
    private static final short EX_BITS = 1;
    private static final short IMSI_CLASS_BITS = 1;
    private static final short IMSI_ADDR_NUM_BITS = 3;
    private static final short MCC_BITS = 10;
    private static final short IMSI_11_12_BITS = 7;
    private static final short IMSI_S2_BITS = 10;
    private static final short IMSI_S1_OCTET_BITS = 8;   // IMSI_S1 is read as three octets
    private static final short ACCOLC_BITS = 4;
    private static final short RESERVED_BITS = 1;

    // EF_MDN record (C.S0023-D §3.4.34): digit count, 8 bytes of digits,
    // then NUMBER_TYPE/PLAN and PI/SI.
    private static final short MDN_DIGIT_BYTES = 8;
    private static final short MDN_MAX_DIGITS = (short) (MDN_DIGIT_BYTES * 2);
    private static final short MDN_TYPE_PLAN_OFF = 9;
    private static final short MDN_PI_SI_OFF = 10;
    private static final short MDN_RECORD_LEN = 11;

    private static final short IMSI_EF_LEN = 10;
    private static final short BLOCK_BUF_LEN = 16;

    private final FileSystem fs;
    private final short[] bit;        // [0] = current bit offset

    public OtaNam(FileSystem fileSystem) {
        fs = fileSystem;
        bit = new short[1];
    }

    // MSB-first bit reader over an external buffer

    private byte[] rdBuf;
    private short rdBase;

    private void start(byte[] buf, short off) {
        rdBuf = buf;
        rdBase = off;
        bit[0] = 0;
    }

    /** Read up to 16 bits, MSB first, returning them right-aligned. */
    private short readBits(short n) {
        short v = 0;
        for (short i = 0; i < n; i++) {
            short p = (short) (bit[0] + i);
            short bytePos = (short) (rdBase + (p >> 3));
            short b = (short) ((rdBuf[bytePos] >> (7 - (p & 7))) & 1);
            v = (short) ((v << 1) | b);
        }
        bit[0] += n;
        return v;
    }

    // Decode block -> EFs (Download / Commit)

    /** Decode a NAM block of the given id into its EFs. Returns true when
     *  the block id is supported. */
    public boolean decode(byte blockId, byte[] buf, short off, short len) {
        switch (blockId) {
            case BLOCK_MDN:
                decodeMdn(buf, off, len);
                return true;
            case BLOCK_CDMA_NAM:
            case BLOCK_CDMA_ANALOG_NAM:
                decodeCdmaNam(blockId, buf, off);
                return true;
            case BLOCK_IMSI_T:
                decodeImsiT(buf, off);
                return true;
            default:
                return false;
        }
    }

    /** Mobile Directory Number block (§4.5.2.2): N_DIGITS(4) then 4-bit
     *  digits, into EF_MDN (digit count in byte 1, digits two per byte
     *  from byte 2, low nibble first, unused nibbles 'F'). */
    private void decodeMdn(byte[] buf, short off, short len) {
        Ef ef = fs.find(Const.FID_EF_MDN);
        if (ef == null || ef.recordLength == 0) return;
        start(buf, off);
        short n = readBits(N_DIGITS_BITS);
        short base = ef.recordOffset((short) 1);
        short rl = (short) (ef.recordLength & 0xFF);
        ef.pool[base] = (byte) n;
        for (short i = 0; i < MDN_DIGIT_BYTES; i++) ef.pool[(short) (base + 1 + i)] = (byte) 0xFF;
        for (short d = 0; d < n && d < MDN_MAX_DIGITS; d++) {
            short digit = readBits(DIGIT_BITS);
            short bytePos = (short) (base + 1 + (d >> 1));
            if ((d & 1) == 0) {
                ef.pool[bytePos] = (byte) ((ef.pool[bytePos] & 0xF0) | (digit & 0x0F));
            } else {
                ef.pool[bytePos] = (byte) ((ef.pool[bytePos] & 0x0F) | ((digit & 0x0F) << 4));
            }
        }
        if (rl >= MDN_RECORD_LEN) {
            ef.pool[(short) (base + MDN_TYPE_PLAN_OFF)] = 0x00;
            ef.pool[(short) (base + MDN_PI_SI_OFF)] = 0x00;
        }
    }

    /** CDMA NAM Download block (§4.5.2.3) into EF_IMSI_M and EF_ACCOLC. */
    private void decodeCdmaNam(byte blockId, byte[] buf, short off) {
        start(buf, off);
        if (blockId == BLOCK_CDMA_ANALOG_NAM) {
            readBits(FIRSTCHP_BITS);
            readBits(HOME_SID_BITS);
            readBits(EX_BITS);
        }
        short imsiClass = readBits(IMSI_CLASS_BITS);
        short addrNum = readBits(IMSI_ADDR_NUM_BITS);
        short mcc = readBits(MCC_BITS);
        short digits1112 = readBits(IMSI_11_12_BITS);
        short s2 = readBits(IMSI_S2_BITS);
        short s1hi = readBits(IMSI_S1_OCTET_BITS);
        short s1mid = readBits(IMSI_S1_OCTET_BITS);
        short s1lo = readBits(IMSI_S1_OCTET_BITS);
        short accolc = readBits(ACCOLC_BITS);
        writeImsi(Const.FID_EF_IMSI_M, imsiClass, addrNum, mcc, digits1112,
                  s2, s1hi, s1mid, s1lo);
        Ef acc = fs.find(Const.FID_EF_ACCOLC);
        if (acc != null && acc.capacity() >= 1) {
            acc.pool[acc.poolOffset] = (byte) (accolc & 0x0F);
        }
    }

    /** IMSI_T block (§4.5.2.4) into EF_IMSI_T. */
    private void decodeImsiT(byte[] buf, short off) {
        start(buf, off);
        short imsiClass = readBits(IMSI_CLASS_BITS);
        short addrNum = readBits(IMSI_ADDR_NUM_BITS);
        short mcc = readBits(MCC_BITS);
        short digits1112 = readBits(IMSI_11_12_BITS);
        short s2 = readBits(IMSI_S2_BITS);
        short s1hi = readBits(IMSI_S1_OCTET_BITS);
        short s1mid = readBits(IMSI_S1_OCTET_BITS);
        short s1lo = readBits(IMSI_S1_OCTET_BITS);
        writeImsi(Const.FID_EF_IMSI_T, imsiClass, addrNum, mcc, digits1112,
                  s2, s1hi, s1mid, s1lo);
    }

    /**
     * Write the IMSI EF per C.S0065-B §5.2.2: class in byte 1, IMSI_S2 in
     * bytes 2-3 (little-endian), IMSI_S1 in bytes 4-6 (little-endian),
     * 11_12 in byte 7, ADDR_NUM + programmed in byte 8, MCC in bytes 9-10.
     */
    private void writeImsi(short fid, short imsiClass, short addrNum, short mcc,
                           short digits1112, short s2, short s1hi, short s1mid, short s1lo) {
        Ef ef = fs.find(fid);
        if (ef == null || ef.capacity() < IMSI_EF_LEN) return;
        short o = ef.poolOffset;
        ef.pool[o] = (byte) (imsiClass & 0x01);
        ef.pool[(short) (o + 1)] = (byte) (s2 & 0xFF);
        ef.pool[(short) (o + 2)] = (byte) ((s2 >> 8) & 0x03);
        ef.pool[(short) (o + 3)] = (byte) (s1lo & 0xFF);
        ef.pool[(short) (o + 4)] = (byte) (s1mid & 0xFF);
        ef.pool[(short) (o + 5)] = (byte) (s1hi & 0xFF);
        ef.pool[(short) (o + 6)] = (byte) (digits1112 & 0x7F);
        ef.pool[(short) (o + 7)] = (byte) ((addrNum & 0x07) | 0x80);  // programmed
        ef.pool[(short) (o + 8)] = (byte) (mcc & 0xFF);
        ef.pool[(short) (o + 9)] = (byte) ((mcc >> 8) & 0x03);
    }

    // Encode block from EFs (Configuration Request)

    private byte[] wrBuf;
    private short wrBase;

    private void startWrite(byte[] buf, short off) {
        wrBuf = buf;
        wrBase = off;
        bit[0] = 0;
        Util.arrayFillNonAtomic(buf, off, BLOCK_BUF_LEN, (byte) 0);
    }

    private void writeBits(short value, short n) {
        for (short i = 0; i < n; i++) {
            short p = (short) (bit[0] + i);
            short bytePos = (short) (wrBase + (p >> 3));
            short v = (short) ((value >> (n - 1 - i)) & 1);
            if (v != 0) wrBuf[bytePos] |= (byte) (1 << (7 - (p & 7)));
        }
        bit[0] += n;
    }

    /** Encode a NAM block from the EFs. Returns the block length in bytes,
     *  or -1 when the block id is not encodable. */
    public short encode(byte blockId, byte[] out, short off) {
        if (blockId == BLOCK_MDN) return encodeMdn(out, off);
        if (blockId == BLOCK_IMSI_T) return encodeImsiT(out, off);
        return -1;
    }

    /** IMSI_T block (§4.5.2.4) from EF_IMSI_T, the inverse of decodeImsiT. */
    private short encodeImsiT(byte[] out, short off) {
        Ef ef = fs.find(Const.FID_EF_IMSI_T);
        if (ef == null || ef.capacity() < IMSI_EF_LEN) return -1;
        short o = ef.poolOffset;
        short s2 = (short) (((ef.pool[(short) (o + 2)] & 0x03) << 8) | (ef.pool[(short) (o + 1)] & 0xFF));
        short mcc = (short) (((ef.pool[(short) (o + 9)] & 0x03) << 8) | (ef.pool[(short) (o + 8)] & 0xFF));
        startWrite(out, off);
        writeBits((short) (ef.pool[o] & 0x01), IMSI_CLASS_BITS);
        writeBits((short) (ef.pool[(short) (o + 7)] & 0x07), IMSI_ADDR_NUM_BITS);
        writeBits(mcc, MCC_BITS);
        writeBits((short) (ef.pool[(short) (o + 6)] & 0x7F), IMSI_11_12_BITS);
        writeBits(s2, IMSI_S2_BITS);
        writeBits((short) (ef.pool[(short) (o + 5)] & 0xFF), IMSI_S1_OCTET_BITS);
        writeBits((short) (ef.pool[(short) (o + 4)] & 0xFF), IMSI_S1_OCTET_BITS);
        writeBits((short) (ef.pool[(short) (o + 3)] & 0xFF), IMSI_S1_OCTET_BITS);
        writeBits((short) 0, RESERVED_BITS);
        return (short) (((short) (bit[0] + 7)) >> 3);
    }

    private short encodeMdn(byte[] out, short off) {
        Ef ef = fs.find(Const.FID_EF_MDN);
        if (ef == null || ef.recordLength == 0) return -1;
        short base = ef.recordOffset((short) 1);
        short n = (short) (ef.pool[base] & 0x0F);
        startWrite(out, off);
        writeBits(n, N_DIGITS_BITS);
        for (short d = 0; d < n && d < MDN_MAX_DIGITS; d++) {
            short bytePos = (short) (base + 1 + (d >> 1));
            short digit = ((d & 1) == 0)
                    ? (short) (ef.pool[bytePos] & 0x0F)
                    : (short) ((ef.pool[bytePos] >> 4) & 0x0F);
            writeBits(digit, DIGIT_BITS);
        }
        return (short) (((short) (bit[0] + 7)) >> 3);
    }
}
