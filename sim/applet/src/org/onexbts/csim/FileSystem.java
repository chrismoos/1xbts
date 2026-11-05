package org.onexbts.csim;

import javacard.framework.Util;

/**
 * EF table and pooled EF storage.
 *
 * The table (EF_META) is loaded from the host with INS_INIT_META after
 * install, 8 bytes per EF:
 *   [0..1] FID
 *   [2]    structure: 1 transparent, 2 linear fixed, 6 cyclic
 *   [3]    SFI (0 = none)
 *   [4..5] file size, big-endian
 *   [6]    record length (0 for transparent)
 *   [7]    access conditions: hi nibble READ, lo nibble UPDATE
 *          (0 ALW, 1 PIN1, 2 ADM, 3 NEVER, 4 PIN2)
 *
 * The JC 3.0.4 converter on the sysmoISIM-SJA5 corrupts static byte[]
 * initializers past about 240 bytes, so the table cannot be compiled in.
 * Entries are read through Util.arrayCopyNonAtomic into a small scratch
 * buffer.
 *
 * All EF content lives in one byte[] (efDataPool) addressed through a
 * per-EF offset table, and a single Ef describes whichever EF
 * was looked up last. Together that keeps the persistent object count
 * under the platform's allocation cap.
 */
public final class FileSystem {

    public static final byte MAX_CHANNELS = 4;

    public static final byte NUM_EFS = 104;
    private static final byte STRIDE = 8;
    // Field offsets within an EF_META entry.
    private static final byte META_FID = 0;
    private static final byte META_STRUCTURE = 2;
    private static final byte META_SFI = 3;
    private static final byte META_SIZE = 4;
    private static final byte META_REC_LEN = 6;
    private static final byte META_AC = 7;
    // FID high byte of an EF under the CSIM ADF. MF-level EFs use 2F.
    private static final byte FID_HI_ADF = 0x6F;
    public static final byte SPLIT = 50;
    private static final short LO_LEN = (short) (SPLIT * STRIDE);
    private static final short HI_LEN = (short) ((NUM_EFS - SPLIT) * STRIDE);
    private static final short TOTAL_LEN = (short) (NUM_EFS * STRIDE);
    // Allocated by the first INIT_META chunk, not at install, to keep the
    // install() allocation inside the platform's commit capacity.
    private byte[] EF_META_LO;
    private byte[] EF_META_HI;

    private static final byte NONE = (byte) 0xFF;
    private static final byte STATE_DEACTIVATED = (byte) 0x01;

    // EF_DIR application template, TS 102 221 §13.1: 61 L 4F Lc AID 50 Ll label.
    private static final byte TAG_APP_TEMPLATE = (byte) 0x61;
    private static final byte TAG_AID = (byte) 0x4F;
    private static final byte TAG_LABEL = (byte) 0x50;
    private static final byte[] APP_LABEL = { (byte) 'C', (byte) 'S', (byte) 'I', (byte) 'M' };

    private static final short TMSI_CODE_OFFSET = 9;
    private static final short TMSI_CODE_LEN = 4;
    private static final short MMSN_ZERO_PREFIX = 3;

    private final byte[] currentEfIdx;
    private boolean metaLoaded;
    private byte[] efDataPool;
    private byte[] efDataOffsets;     // NUM_EFS big-endian shorts
    private byte[] efState;           // per EF, bit 0 = deactivated
    private final Ef flyweight;
    private final byte[] aid;

    private final byte[] fidScratch;
    private final byte[] entryScratch;
    private final byte[] recScratch;

    public FileSystem(byte[] applicationAid) {
        aid = applicationAid;
        flyweight = new Ef();
        currentEfIdx = new byte[MAX_CHANNELS];
        for (byte i = 0; i < MAX_CHANNELS; i++) {
            currentEfIdx[i] = NONE;
        }
        fidScratch = new byte[(short) 2];
        entryScratch = new byte[(short) STRIDE];
        recScratch = new byte[(short) 3];
        metaLoaded = false;
    }

    private byte[] _chunkOf(short off) {
        return (off < LO_LEN) ? EF_META_LO : EF_META_HI;
    }

    private short _localOff(short off) {
        return (off < LO_LEN) ? off : (short) (off - LO_LEN);
    }

    /** Receive a chunk of EF_META from the host. The last chunk sets
     *  finished=true, which builds the data pool and applies defaults. */
    public void initMetaChunk(byte[] src, short srcOff,
                              short dstOff, short length,
                              boolean finished) {
        if (EF_META_LO == null) {
            EF_META_LO = new byte[LO_LEN];
            EF_META_HI = new byte[HI_LEN];
        }
        if (dstOff < LO_LEN && (short) (dstOff + length) > LO_LEN) {
            short loPart = (short) (LO_LEN - dstOff);
            Util.arrayCopyNonAtomic(src, srcOff, EF_META_LO, dstOff, loPart);
            Util.arrayCopyNonAtomic(src, (short) (srcOff + loPart),
                                    EF_META_HI, (short) 0,
                                    (short) (length - loPart));
        } else {
            Util.arrayCopyNonAtomic(src, srcOff,
                                    _chunkOf(dstOff), _localOff(dstOff),
                                    length);
        }
        if (finished) {
            initDataPool();
            metaLoaded = true;
        }
    }

    private void loadEntry(byte idx) {
        short base = (short) (idx * STRIDE);
        Util.arrayCopyNonAtomic(_chunkOf(base), _localOff(base),
                                entryScratch, (short) 0, STRIDE);
    }

    private short entryFid() {
        return Util.getShort(entryScratch, META_FID);
    }

    private short entrySize() {
        return Util.getShort(entryScratch, META_SIZE);
    }

    /** Size the pool, record per-EF offsets, apply the pre-personalization
     *  defaults of C.S0065-B Annex E and TS 102 221, and seed EF_DIR. */
    private void initDataPool() {
        short total = 0;
        for (byte i = 0; i < NUM_EFS; i++) {
            loadEntry(i);
            total = (short) (total + entrySize());
        }
        efDataPool = new byte[total];
        efDataOffsets = new byte[(short) (NUM_EFS * 2)];
        efState = new byte[NUM_EFS];

        short cur = 0;
        for (byte i = 0; i < NUM_EFS; i++) {
            loadEntry(i);
            short fid = entryFid();
            short sz = entrySize();
            short recLen = (short) (entryScratch[META_REC_LEN] & 0xFF);
            efDataOffsets[(short) (i * 2)] = (byte) ((cur >> 8) & 0xFF);
            efDataOffsets[(short) (i * 2 + 1)] = (byte) (cur & 0xFF);

            if (sz > 0 && fillWithFF(fid)) {
                Util.arrayFillNonAtomic(efDataPool, cur, sz, (byte) 0xFF);
                short zeroPrefix = zeroPrefixPerRecord(fid);
                if (zeroPrefix > 0 && recLen > 0) {
                    for (short r = 0; r < sz; r = (short) (r + recLen)) {
                        Util.arrayFillNonAtomic(efDataPool, (short) (cur + r),
                                                zeroPrefix, (byte) 0);
                    }
                }
            }
            // EF_TMSI default (Annex E): TMSI_CODE bytes 10..13 are 'FF'.
            if (fid == Const.FID_EF_TMSI && sz >= (short) (TMSI_CODE_OFFSET + TMSI_CODE_LEN)) {
                Util.arrayFillNonAtomic(efDataPool, (short) (cur + TMSI_CODE_OFFSET),
                                        TMSI_CODE_LEN, (byte) 0xFF);
            }
            if (fid == Const.FID_EF_DIR && recLen > 0) {
                seedEfDir(cur, recLen);
            }
            cur = (short) (cur + sz);
        }
        syncSpcStatus();
    }

    /** EFs whose unpersonalized content is 'FF' (C.S0065-B Annex E, the
     *  per-EF "unused bytes shall be set to 'FF'" rules, TS 102 221 §13). */
    private static boolean fillWithFF(short fid) {
        switch (fid) {
            case Const.FID_EF_DIR:
            case Const.FID_EF_PL:
            case Const.FID_EF_LI:
            case Const.FID_EF_FDN:
            case Const.FID_EF_SMS:
            case Const.FID_EF_SMSP:
            case Const.FID_EF_SMSS:
            case Const.FID_EF_SPN:
            case Const.FID_EF_ECC:
            case Const.FID_EF_CSSPR:
            case Const.FID_EF_BCSMSPREF:
            case Const.FID_EF_BCSMSTABLE:
            case Const.FID_EF_BCSMSP:
            case Const.FID_EF_MMSN:
            case Const.FID_EF_EXT8:
            case Const.FID_EF_MMSICP:
            case Const.FID_EF_MMSUP:
            case Const.FID_EF_MMSUCP:
            case Const.FID_EF_GID1:
            case Const.FID_EF_GID2:
            case Const.FID_EF_HOME_TAG:
            case Const.FID_EF_GROUP_TAG:
            case Const.FID_EF_SPECIFIC_TAG:
            case Const.FID_EF_CALL_PROMPT:
            case Const.FID_EF_SDN:
            case Const.FID_EF_EXT2:
            case Const.FID_EF_EXT3:
            case Const.FID_EF_ICI:
            case Const.FID_EF_OCI:
            case Const.FID_EF_EXT5:
            case Const.FID_EF_CCP2:
            case Const.FID_EF_MODEL:
            case Const.FID_EF_WAPBROWSER_BM:
                return true;
            default:
                return false;
        }
    }

    /** Record EFs whose default record starts with zero bytes: EF_SMS
     *  and EF_BCSMStable '00 FF..FF', EF_MMSN '00 00 00 FF..FF'. */
    private static short zeroPrefixPerRecord(short fid) {
        switch (fid) {
            case Const.FID_EF_SMS:
            case Const.FID_EF_BCSMSTABLE:
                return 1;
            case Const.FID_EF_MMSN:
                return MMSN_ZERO_PREFIX;
            default:
                return 0;
        }
    }

    private void seedEfDir(short poolOff, short recLen) {
        short bodyLen = (short) (2 + aid.length + 2 + APP_LABEL.length);
        if ((short) (bodyLen + 2) > recLen) return;
        short o = poolOff;
        recScratch[0] = TAG_APP_TEMPLATE;
        recScratch[1] = (byte) bodyLen;
        recScratch[2] = TAG_AID;
        Util.arrayCopyNonAtomic(recScratch, (short) 0, efDataPool, o, (short) 3);
        o = (short) (o + 3);
        recScratch[0] = (byte) aid.length;
        Util.arrayCopyNonAtomic(recScratch, (short) 0, efDataPool, o, (short) 1);
        o = (short) (o + 1);
        Util.arrayCopyNonAtomic(aid, (short) 0, efDataPool, o, (short) aid.length);
        o = (short) (o + aid.length);
        recScratch[0] = TAG_LABEL;
        recScratch[1] = (byte) APP_LABEL.length;
        Util.arrayCopyNonAtomic(recScratch, (short) 0, efDataPool, o, (short) 2);
        o = (short) (o + 2);
        Util.arrayCopyNonAtomic(APP_LABEL, (short) 0, efDataPool, o, (short) APP_LABEL.length);
    }

    /** EF_SPCS b1 is 1 when EF_SPC holds anything other than '00 00 00'
     *  (C.S0065-B §5.2.37). EF_SPCS cannot be updated over the interface,
     *  so it is recomputed after every write to EF_SPC. */
    public void syncSpcStatus() {
        byte spcIdx = indexByFid(Const.FID_EF_SPC);
        byte spcsIdx = indexByFid(Const.FID_EF_SPCS);
        if (spcIdx < 0 || spcsIdx < 0) return;
        short spcOff = dataOffset(spcIdx);
        Util.arrayCopyNonAtomic(efDataPool, spcOff, recScratch, (short) 0, (short) 3);
        byte status = (byte) ((recScratch[0] | recScratch[1] | recScratch[2]) != 0 ? 1 : 0);
        efDataPool[dataOffset(spcsIdx)] = status;
    }

    public boolean isMetaLoaded() {
        return metaLoaded;
    }

    public short poolLength() {
        return (efDataPool == null) ? (short) 0 : (short) efDataPool.length;
    }

    /** Diagnostic: byte sum of EF_META or the data pool read with direct
     *  array indexing. Separates a platform indexing fault from a copy
     *  fault when compared with {@link #checksumCopy}. */
    public short checksumDirect(boolean pool) {
        short sum = 0;
        if (EF_META_LO == null) return 0;
        if (pool) {
            if (efDataPool == null) return 0;
            for (short i = 0; i < (short) efDataPool.length; i++) {
                sum = (short) (sum + (efDataPool[i] & 0xFF));
            }
            return sum;
        }
        for (short i = 0; i < LO_LEN; i++) sum = (short) (sum + (EF_META_LO[i] & 0xFF));
        for (short i = 0; i < HI_LEN; i++) sum = (short) (sum + (EF_META_HI[i] & 0xFF));
        return sum;
    }

    /** Diagnostic: the same sum computed through Util.arrayCopyNonAtomic. */
    public short checksumCopy(boolean pool) {
        short sum = 0;
        if (EF_META_LO == null) return 0;
        byte[] src = pool ? efDataPool : null;
        short len = pool ? poolLength() : TOTAL_LEN;
        for (short off = 0; off < len; off = (short) (off + STRIDE)) {
            short n = (short) (len - off);
            if (n > STRIDE) n = STRIDE;
            if (pool) {
                Util.arrayCopyNonAtomic(src, off, entryScratch, (short) 0, n);
            } else {
                dumpMeta(off, n, entryScratch, (short) 0);
            }
            for (short k = 0; k < n; k++) sum = (short) (sum + (entryScratch[k] & 0xFF));
        }
        return sum;
    }

    public short metaSize() {
        return TOTAL_LEN;
    }

    /** Debug: read a slice of EF_META into dst. Returns bytes copied. */
    public short dumpMeta(short offset, short length,
                          byte[] dst, short dstOff) {
        if (EF_META_LO == null || offset >= TOTAL_LEN) return 0;
        if ((short) (offset + length) > TOTAL_LEN) {
            length = (short) (TOTAL_LEN - offset);
        }
        if (offset < LO_LEN && (short) (offset + length) > LO_LEN) {
            short loPart = (short) (LO_LEN - offset);
            Util.arrayCopyNonAtomic(EF_META_LO, offset, dst, dstOff, loPart);
            Util.arrayCopyNonAtomic(EF_META_HI, (short) 0,
                                    dst, (short) (dstOff + loPart),
                                    (short) (length - loPart));
        } else {
            Util.arrayCopyNonAtomic(_chunkOf(offset), _localOff(offset),
                                    dst, dstOff, length);
        }
        return length;
    }

    public Ef find(short fid) {
        if (!metaLoaded) return null;
        byte idx = indexByFid(fid);
        if (idx < 0) return null;
        return materialize(idx);
    }

    /**
     * SFI lookup per TS 102 221 §8.9, scoped to the current DF. SFIs are
     * unique within a DF, not across the card: SFI 02 at the MF is
     * EF_ICCID and SFI 02 in the CSIM ADF is EF_CSIM_ST. ADF-level EFs
     * are the '6Fxx' range, MF-level EFs are '2Fxx'.
     */
    public Ef findBySfi(byte sfi, boolean inAdf) {
        if (!metaLoaded || sfi == 0) return null;
        for (byte i = 0; i < NUM_EFS; i++) {
            loadEntry(i);
            if (entryScratch[META_SFI] != sfi) continue;
            boolean isAdfEf = (entryScratch[META_FID] == FID_HI_ADF);
            if (inAdf == isAdfEf) {
                return materialize(i);
            }
        }
        return null;
    }

    public static boolean isAdfEf(short fid) {
        return (byte) ((fid >> 8) & 0xFF) == (byte) 0x6F;
    }

    public void select(byte channel, Ef ef) {
        currentEfIdx[channel] = (ef == null) ? NONE : ef.idx;
    }

    public Ef current(byte channel) {
        byte idx = currentEfIdx[channel];
        if (idx == NONE) return null;
        return materialize(idx);
    }

    public void closeChannel(byte channel) {
        currentEfIdx[channel] = NONE;
    }

    public void setDeactivated(Ef ef, boolean deactivated) {
        efState[ef.idx] = deactivated ? STATE_DEACTIVATED : (byte) 0;
        ef.deactivated = deactivated;
    }

    /** DEACTIVATE FILE condition of an EF (C.S0065-B §5.2 "INVALIDATE"). */
    public static byte deactivateAc(short fid) {
        switch (fid) {
            case Const.FID_EF_RUIMID:
            case Const.FID_EF_SPCS:
            case Const.FID_EF_SF_EUIMID:
                return Const.AC_NEVER;
            default:
                return Const.AC_ADM;
        }
    }

    /** INCREASE condition of a cyclic EF. C.S0065-B §5.2 lists INCREASE
     *  for EF_COUNT only, so the other cyclic EFs never allow it. */
    public static byte increaseAc(short fid, byte updateAc) {
        return fid == Const.FID_EF_COUNT ? updateAc : Const.AC_NEVER;
    }

    /** ACTIVATE FILE condition of an EF (C.S0065-B §5.2 "REHABILITATE"). */
    public static byte activateAc(short fid) {
        switch (fid) {
            case Const.FID_EF_IMSI_M:
            case Const.FID_EF_IMSI_T:
            case Const.FID_EF_TMSI:
                return Const.AC_PIN1;
            case Const.FID_EF_RUIMID:
            case Const.FID_EF_SPCS:
            case Const.FID_EF_SF_EUIMID:
                return Const.AC_NEVER;
            default:
                return Const.AC_ADM;
        }
    }

    // helpers

    private byte indexByFid(short fid) {
        fidScratch[0] = (byte) ((fid >> 8) & 0xFF);
        fidScratch[1] = (byte) (fid & 0xFF);
        for (byte i = 0; i < NUM_EFS; i++) {
            short base = (short) (i * STRIDE);
            if (Util.arrayCompare(_chunkOf(base), _localOff(base),
                                  fidScratch, (short) 0, (short) 2) == 0) {
                return i;
            }
        }
        return -1;
    }

    private short dataOffset(byte idx) {
        return (short) (((efDataOffsets[(short) (idx * 2)] & 0xFF) << 8)
                | (efDataOffsets[(short) (idx * 2 + 1)] & 0xFF));
    }

    private Ef materialize(byte idx) {
        loadEntry(idx);
        short fid = entryFid();
        byte structure = entryScratch[META_STRUCTURE];
        byte sfi = entryScratch[META_SFI];
        short size = entrySize();
        byte recLen = entryScratch[META_REC_LEN];
        byte ac = entryScratch[META_AC];
        byte readAc = (byte) ((ac >> 4) & 0x0F);
        byte updateAc = (byte) (ac & 0x0F);
        boolean deactivated = (efState[idx] & STATE_DEACTIVATED) != 0;
        flyweight.set(idx, fid, structure, size, recLen, sfi,
                      readAc, updateAc, deactivated, efDataPool, dataOffset(idx));
        return flyweight;
    }
}
