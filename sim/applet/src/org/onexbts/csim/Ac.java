package org.onexbts.csim;

import javacard.framework.ISOException;
import javacard.framework.JCSystem;
import javacard.framework.OwnerPIN;

/**
 * Access control: PIN1, PIN2, PUK1, PUK2, ADM1, per-channel
 * verification state, and PIN1 enable/disable.
 *
 * - PIN1 (key ref 0x01): user PIN. May be disabled via DISABLE PIN
 *   (TS 102 221 §11.1.10), in which case ac.check(AC_PIN1, ch) passes
 *   without a prior VERIFY.
 * - PIN2 (key ref 0x81): second user PIN, gates FDN / SDN / a few
 *   premium EFs (TS 102 221 §9.5.2).
 * - PUK1 / PUK2: unblock keys for PIN1 / PIN2. RESET RETRY COUNTER
 *   (TS 102 221 §11.1.13) consumes the matching PUK.
 * - ADM1 (key ref 0x0A): admin key for personalization. No
 *   enable/disable. Its verification flag is per channel, like the PINs.
 *
 * Per TS 102 221 §11.1.9.1, PIN validation is per-logical-channel.
 * OwnerPIN's own isValidated() is global, so verification is tracked
 * separately in three byte arrays indexed by channel. OwnerPIN.check()
 * is still used for the actual try-counter-managed comparison.
 */
public final class Ac {

    public final OwnerPIN pin1;
    public final OwnerPIN pin2;
    public final OwnerPIN puk1;
    public final OwnerPIN puk2;
    public final OwnerPIN adm1;

    private final boolean[] pin1Enabled;

    // Per-channel verification flags, 1 = verified on that channel. They
    // must be CLEAR_ON_RESET: the JCRE does not call deselect() on a card
    // reset or power loss, so persistent flags would keep a PIN verified
    // into the next session.
    private final byte[] pin1OnChannel;
    private final byte[] pin2OnChannel;
    private final byte[] admOnChannel;

    public Ac() {
        pin1 = new OwnerPIN(Const.PIN_MAX_TRIES, Const.PIN_MAX_SIZE);
        pin2 = new OwnerPIN(Const.PIN_MAX_TRIES, Const.PIN_MAX_SIZE);
        puk1 = new OwnerPIN(Const.PUK_MAX_TRIES, Const.PUK_MAX_SIZE);
        puk2 = new OwnerPIN(Const.PUK_MAX_TRIES, Const.PUK_MAX_SIZE);
        adm1 = new OwnerPIN(Const.ADM_MAX_TRIES, Const.ADM_MAX_SIZE);
        pin1.update(Const.DEFAULT_PIN1, (short) 0, (byte) Const.DEFAULT_PIN1.length);
        pin2.update(Const.DEFAULT_PIN2, (short) 0, (byte) Const.DEFAULT_PIN2.length);
        puk1.update(Const.DEFAULT_PUK1, (short) 0, (byte) Const.DEFAULT_PUK1.length);
        puk2.update(Const.DEFAULT_PUK2, (short) 0, (byte) Const.DEFAULT_PUK2.length);
        adm1.update(Const.DEFAULT_ADM1, (short) 0, (byte) Const.DEFAULT_ADM1.length);
        pin1Enabled = new boolean[1];
        pin1Enabled[0] = true;
        pin1OnChannel = JCSystem.makeTransientByteArray(FileSystem.MAX_CHANNELS, JCSystem.CLEAR_ON_RESET);
        pin2OnChannel = JCSystem.makeTransientByteArray(FileSystem.MAX_CHANNELS, JCSystem.CLEAR_ON_RESET);
        admOnChannel = JCSystem.makeTransientByteArray(FileSystem.MAX_CHANNELS, JCSystem.CLEAR_ON_RESET);
    }

    public boolean isPin1Enabled() {
        return pin1Enabled[0];
    }

    public void setPin1Enabled(boolean enabled) {
        pin1Enabled[0] = enabled;
    }

    public void setPin1Verified(byte channel, boolean v) {
        pin1OnChannel[channel] = v ? (byte) 1 : (byte) 0;
    }

    public void setPin2Verified(byte channel, boolean v) {
        pin2OnChannel[channel] = v ? (byte) 1 : (byte) 0;
    }

    public void setAdmVerified(byte channel, boolean v) {
        admOnChannel[channel] = v ? (byte) 1 : (byte) 0;
    }

    public boolean isAdmVerified(byte channel) {
        return admOnChannel[channel] != 0;
    }

    /** Clear all verification state for the given channel. */
    public void clearChannel(byte channel) {
        pin1OnChannel[channel] = 0;
        pin2OnChannel[channel] = 0;
        admOnChannel[channel] = 0;
    }

    /** Clear verification state across every channel. */
    public void clearAllChannels() {
        for (byte c = 0; c < FileSystem.MAX_CHANNELS; c++) {
            clearChannel(c);
        }
    }

    /** Throws 6982 if the AC for `op` is not satisfied on `channel`. */
    public void check(byte ac, byte channel) {
        switch (ac) {
            case Const.AC_ALWAYS: return;
            case Const.AC_PIN1:
                if (!pin1Enabled[0] || pin1OnChannel[channel] != 0) return;
                ISOException.throwIt(Const.SW_SECURITY_NOT_SAT);
                return;
            case Const.AC_PIN2:
                if (pin2OnChannel[channel] != 0) return;
                ISOException.throwIt(Const.SW_SECURITY_NOT_SAT);
                return;
            case Const.AC_ADM:
                if (admOnChannel[channel] != 0) return;
                ISOException.throwIt(Const.SW_SECURITY_NOT_SAT);
                return;
            case Const.AC_NEVER:
            default:
                ISOException.throwIt(Const.SW_SECURITY_NOT_SAT);
        }
    }
}
