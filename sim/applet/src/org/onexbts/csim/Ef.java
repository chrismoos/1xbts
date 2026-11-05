package org.onexbts.csim;

/**
 * Describes one EF at a time.
 *
 * One instance exists per FileSystem. Its fields are overwritten by
 * every FileSystem.find()/current()/findBySfi() call, so a caller must
 * finish with one EF before looking up another. The sysmoISIM-SJA5 caps
 * persistent object allocation around 50 instances, which rules out one
 * object per EF.
 *
 * The file content lives in {@code pool[poolOffset .. poolOffset+size)}.
 * Bytes outside that range belong to other EFs.
 */
public final class Ef {

    public byte idx;
    public short fid;
    public byte structure;        // FS_TRANSPARENT / FS_LINEAR_FIXED / FS_CYCLIC
    public byte sfi;              // 0 = no SFI assigned
    public byte recordLength;     // 0 for transparent
    public short size;
    public byte readAc;
    public byte updateAc;
    public boolean deactivated;

    public byte[] pool;
    public short poolOffset;

    public void set(byte idx, short fid, byte structure, short size,
                    byte recordLength, byte sfi, byte readAc, byte updateAc,
                    boolean deactivated, byte[] pool, short poolOffset) {
        this.idx = idx;
        this.fid = fid;
        this.structure = structure;
        this.sfi = sfi;
        this.recordLength = recordLength;
        this.size = size;
        this.readAc = readAc;
        this.updateAc = updateAc;
        this.deactivated = deactivated;
        this.pool = pool;
        this.poolOffset = poolOffset;
    }

    public short capacity() {
        return size;
    }

    public short recordCount() {
        if (structure == Const.FS_TRANSPARENT || recordLength == 0) return 0;
        return (short) (size / (recordLength & 0xFF));
    }

    /** Pool offset of 1-based record {@code rec}. */
    public short recordOffset(short rec) {
        return (short) (poolOffset + (short) ((short) (rec - 1) * (short) (recordLength & 0xFF)));
    }
}
