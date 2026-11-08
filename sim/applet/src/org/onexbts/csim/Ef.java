package org.onexbts.csim;

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
    private static final short BYTE_MASK = 0xFF;
    private byte recordHead;

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

    // Handset writes must commit the head and new record in one transaction.
    public void advanceCyclic() {
        short head = (short) (recordHead & BYTE_MASK);
        short next = head == 0 ? (short) (recordCount() - 1) : (short) (head - 1);
        if (head != next) recordHead = (byte) next;
    }

    public short recordOffset(short rec) {
        short slot = (short) (rec - 1);
        if (structure == Const.FS_CYCLIC) {
            slot = (short) (slot + (recordHead & BYTE_MASK));
            short count = recordCount();
            if (slot >= count) slot = (short) (slot - count);
        }
        return (short) (poolOffset + (short) (slot * (short) (recordLength & 0xFF)));
    }
}
