import com.licel.jcardsim.smartcardio.CardSimulator;
import com.licel.jcardsim.utils.AIDUtil;
import javacard.framework.AID;
import javax.smartcardio.CommandAPDU;
import javax.smartcardio.ResponseAPDU;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.math.BigInteger;

/**
 * Functional checks of the CSIM applet in jcardsim, APDU by APDU.
 * Covers the TS 102 221 file commands, PIN handling, cyclic EF
 * semantics, file deactivation, Store ESN_MEID_ME and the
 * personalization commands. Each check prints PASS or FAIL. Exit code 1
 * if any check failed. Run with ./gradlew simtest.
 */
public class SimTest {
    static CardSimulator sim;
    static int failures = 0;
    static final byte[] AID_BYTES = hex("A0000003431002F310FFFF89020000FF");
    // S.S0055-A Exhibit 4-3 kernel vectors. AUTN = (SQN 000000000001 ^ AK
    // 594cc7c17c06) || AMF 0001 || MAC-A 6abdc4da73c61b8d.
    static final String AKA_K = "ad1b5a159be86b2ca66c7ae40bba9b9d";
    static final String AKA_RAND = "4b052b20e2a06c8ff700da512b4e111e";
    static final String AKA_AUTN = "594cc7c17c0700016abdc4da73c61b8d";
    static final String AKA_CK = "6efdd832f6ffd4dca84a5496fa6e2993";
    static final String AKA_IK = "c1436525fa607f1792fca89fb2a7bc4a";
    static final String AKA_RES = "d82e282adc13c0f1686566339bf27eb6";

    static byte[] hex(String s) {
        s = s.replace(" ", "");
        byte[] out = new byte[s.length() / 2];
        for (int i = 0; i < out.length; i++) out[i] = (byte) Integer.parseInt(s.substring(2 * i, 2 * i + 2), 16);
        return out;
    }
    static String hex(byte[] b) {
        StringBuilder sb = new StringBuilder();
        for (byte x : b) sb.append(String.format("%02X", x));
        return sb.toString();
    }
    static ResponseAPDU tx(String apdu) {
        return sim.transmitCommand(new CommandAPDU(hex(apdu)));
    }
    static ResponseAPDU tx(byte[] apdu) {
        return sim.transmitCommand(new CommandAPDU(apdu));
    }
    static void check(String name, boolean ok, String detail) {
        System.out.println((ok ? "PASS " : "FAIL ") + name + (ok ? "" : "  -> " + detail));
        if (!ok) failures++;
    }
    static void sw(String name, ResponseAPDU r, int expect) {
        check(name, r.getSW() == expect, String.format("SW=%04X expected %04X data=%s", r.getSW(), expect, hex(r.getData())));
    }
    static void data(String name, ResponseAPDU r, String expectHex) {
        String got = hex(r.getData());
        check(name, r.getSW() == 0x9000 && got.equalsIgnoreCase(expectHex.replace(" ", "")),
              String.format("SW=%04X data=%s expected %s", r.getSW(), got, expectHex.replace(" ", "")));
    }
    static boolean contains(byte[] hay, String needleHex) {
        return hex(hay).contains(needleHex.replace(" ", ""));
    }

    public static void main(String[] args) throws Exception {
        byte[] meta = hex(new String(Files.readAllBytes(Paths.get(args[0]))).trim());
        sim = new CardSimulator();
        AID aid = AIDUtil.create(AID_BYTES);
        sim.installApplet(aid, org.onexbts.csim.Csim.class);

        // SELECT by AID: FCP with DF name and PIN status template.
        ResponseAPDU r = tx(concat("00A40404", AID_BYTES, "00"));
        sw("select ADF", r, 0x9000);
        check("ADF FCP has DF name", contains(r.getData(), "8410" + hex(AID_BYTES)), hex(r.getData()));
        check("ADF FCP has PIN status template (PIN1 enabled)", contains(r.getData(), "C60C9001E0830101830181 83010A"), hex(r.getData()));
        check("ADF FCP has FID 7FFF", contains(r.getData(), "83027FFF"), hex(r.getData()));
        check("ADF FCP has UICC characteristics", contains(r.getData(), "A503800171"), hex(r.getData()));

        // Before INIT_META everything is absent.
        sw("select EF before INIT_META -> 6A82", tx("00A4000C026F22"), 0x6A82);

        // INIT_META needs ADM.
        sw("INIT_META without ADM -> 6982", tx(concat("80FA0000", slice(meta, 0, 100), "")), 0x6982);
        sw("verify ADM1", tx("0020000A080000000000000000"), 0x9000);
        for (int off = 0; off < meta.length; off += 100) {
            int n = Math.min(100, meta.length - off);
            boolean last = off + n >= meta.length;
            byte p1 = (byte) (((off >> 8) & 0x7F) | (last ? 0x80 : 0));
            byte[] apdu = concat(new byte[] { (byte) 0x80, (byte) 0xFA, p1, (byte) (off & 0xFF), (byte) n }, slice(meta, off, n), "");
            sw("INIT_META chunk @" + off, tx(apdu), 0x9000);
        }

        // EF_IMSI_M FCP.
        r = tx("00A40004026F22");
        sw("select EF_IMSI_M", r, 0x9000);
        data("EF_IMSI_M FCP", r, "6240 8202 4121 8302 6F22 8A01 05 AB2C 800101 A406830101950108 800102 A40683010A950108 800108 A40683010A950108 800110 A406830101950108 8002 000A 8801 20");

        // PIN1 gates READ, ADM gates UPDATE.
        // Open a fresh session view: ADM is verified, PIN1 not.
        sw("read IMSI_M without PIN1 -> 6982", tx("00B000000A"), 0x6982);
        sw("verify PIN1 status query -> 63C3", tx("0020000100"), 0x63C3);
        sw("verify PIN1 wrong length -> 6700", tx("002000010431323334"), 0x6700);
        sw("verify PIN1", tx("00200001083132333 4FFFFFFFF".replace(" ", "")), 0x9000);
        data("read IMSI_M default zeros", tx("00B000000A"), "00000000000000000000");
        sw("update IMSI_M (ADM verified)", tx("00D600000A00A1B2C3D4E5F6A7B8C9"), 0x9000);
        data("read IMSI_M back", tx("00B000000A"), "00A1B2C3D4E5F6A7B8C9");
        sw("read binary offset beyond file -> 6B00", tx("00B0000A0A"), 0x6B00);
        sw("update binary too long -> 6700", tx("00D600050600112233445566"), 0x6700);
        data("read via SFI 04", tx("00B084000A"), "00A1B2C3D4E5F6A7B8C9");

        // EF_EPRL is 12288 bytes: offsets above 255 reach the whole file.
        sw("select EF_EPRL", tx("00A4000C026F5A"), 0x9000);
        sw("update EPRL at offset 0x2FF0", tx("00D62FF0040A0B0C0D"), 0x9000);
        data("read EPRL at offset 0x2FF0", tx("00B02FF004"), "0A0B0C0D");
        data("read EPRL last 16 bytes", tx("00B02FF010"), "0A0B0C0D000000000000000000000000");
        sw("read EPRL at offset 0x3000 -> 6B00", tx("00B0300001"), 0x6B00);

        // The APDU trace names the files a terminal selected.
        sw("select EF_AD for the trace", tx("00A4000C026F43"), 0x9000);
        if (!org.onexbts.csim.Variant.DEBUG) {
            sw("hardened: APDU trace dump -> 6D00", tx("80FC000000"), 0x6D00);
            sw("hardened: APDU trace clear -> 6D00", tx("80FD0000"), 0x6D00);
            sw("hardened: EF table dump -> 6D00", tx("80FB000020"), 0x6D00);
        } else {
        r = tx("80FC000000");
        check("APDU trace dump returns head and 254 bytes", r.getSW() == 0x9000 && r.getData().length == 256, String.format("SW=%04X len=%d", r.getSW(), r.getData().length));
        String trace = hex(r.getData()).substring(4);
        r = tx("80FC00FE00");
        check("APDU trace second chunk is 98 bytes", r.getSW() == 0x9000 && r.getData().length == 100, String.format("SW=%04X len=%d", r.getSW(), r.getData().length));
        trace += hex(r.getData()).substring(4);
        check("APDU trace holds SELECT EF_AD with its FID", trace.contains("00A4000C026F4300009000"), trace);
        check("APDU trace holds the SELECT by AID with its prefix", trace.contains("00A4040410A00000039000"), trace);
        sw("clear APDU trace", tx("80FD0000"), 0x9000);
        }

        // EF_RUIMID: UPDATE NEVER, pre-personalization path works.
        sw("select EF_RUIMID", tx("00A4000C026F31"), 0x9000);
        sw("update RUIMID over UPDATE BINARY -> 6982", tx("00D60000080464D5C58000000000"), 0x6982);
        sw("preperso RUIMID", tx("80F46F31080464D5C58000000000"), 0x9000);
        data("read RUIMID", tx("00B0000008"), "0464D5C580000000");
        sw("preperso on record EF -> 6981", tx("80F46F28050000000000"), 0x6981);

        // EF_SPCS follows EF_SPC.
        sw("select SPCS", tx("00A4000C026F46"), 0x9000);
        data("SPCS default 00", tx("00B0000001"), "00");
        sw("select SPC", tx("00A4000C026F33"), 0x9000);
        sw("update SPC", tx("00D6000003123456"), 0x9000);
        sw("select SPCS again", tx("00A4000C026F46"), 0x9000);
        data("SPCS now 01", tx("00B0000001"), "01");
        sw("update SPCS -> 6982 (NEVER)", tx("00D600000100"), 0x6982);

        // Cyclic EF_COUNT: INCREASE and UPDATE RECORD previous.
        r = tx("00A40004026F21");
        sw("select EF_COUNT", r, 0x9000);
        check("EF_COUNT FCP cyclic 10x2", contains(r.getData(), "8205 4621 0002 0A"), hex(r.getData()));
        check("EF_COUNT FCP has expanded security attributes with INCREASE rule",
              contains(r.getData(), "AB37 800101 A406830101950108 800102 A406830101950108 840132 A406830101950108 800108 A40683010A950108 800110 A40683010A950108"), hex(r.getData()));
        data("read COUNT record 1", tx("00B2010402"), "0000");
        data("INCREASE COUNT by 5", tx("003200000300000500"), "0005 000005");
        data("INCREASE COUNT by 5 with Lc=2", tx("00320000020005"), "000A 0005");
        sw("INCREASE P1 without SFI flag -> 6B00", tx("00320100020005"), 0x6B00);
        sw("INCREASE overflow -> 9850", tx("0032000003FFFFFF"), 0x9850);
        sw("INCREASE overflow in high bytes -> 9850", tx("0032000003010000"), 0x9850);
        data("COUNT still 000A", tx("00B2010402"), "000A");
        sw("UPDATE RECORD previous (cyclic)", tx("00DC0003020007"), 0x9000);
        data("COUNT now 0007", tx("00B2010402"), "0007");
        sw("UPDATE RECORD next on cyclic -> 6B00", tx("00DC0002020008"), 0x6B00);
        sw("UPDATE RECORD absolute on cyclic -> 6B00", tx("00DC0104020008"), 0x6B00);

        // Cyclic EF_ICI with 10 records of 44: rotation and wrap-around reads.
        sw("select EF_ICI", tx("00A4000C026F7C"), 0x9000);
        sw("INCREASE on EF_ICI -> 6982 (INCREASE is NEVER outside EF_COUNT)", tx("00320000020001"), 0x6982);
        r = tx("00A40004026F7C");
        check("EF_ICI FCP states INCREASE never", contains(r.getData(), "840132 9700"), hex(r.getData()));
        check("EF_ICI FCP cyclic 10x44", contains(r.getData(), "8205 4621 002C 0A"), hex(r.getData()));
        sw("select EF_ICI again", tx("00A4000C026F7C"), 0x9000);
        byte[] recA = new byte[44]; java.util.Arrays.fill(recA, (byte) 0xAA);
        byte[] recB = new byte[44]; java.util.Arrays.fill(recB, (byte) 0xBB);
        byte[] recFF = new byte[44]; java.util.Arrays.fill(recFF, (byte) 0xFF);
        sw("ICI write A (previous)", tx(concat("00DC0003", recA, "")), 0x9000);
        sw("ICI write B (previous)", tx(concat("00DC0003", recB, "")), 0x9000);
        data("ICI record 1 is newest (B)", tx("00B201042C"), hex(recB));
        data("ICI record 2 is older (A)", tx("00B202042C"), hex(recA));
        data("ICI record 10 is unused", tx("00B20A042C"), hex(recFF));
        // Absolute reads leave the pointer at record 1 (set by the update).
        data("ICI next from record 1 is record 2", tx("00B200022C"), hex(recA));
        data("ICI previous from record 2 is record 1", tx("00B200032C"), hex(recB));
        data("ICI previous from record 1 wraps to 10", tx("00B200032C"), hex(recFF));
        data("ICI next from record 10 wraps to 1", tx("00B200022C"), hex(recB));
        data("ICI current record is 1", tx("00B200042C"), hex(recB));

        // Linear fixed with 20 records: next past the end is an error.
        sw("select CDMAHOME", tx("00A4000C026F28"), 0x9000);
        sw("CDMAHOME update rec 20", tx("00DC140405160000FF00"), 0x9000);
        data("CDMAHOME read rec 20", tx("00B2140405"), "160000FF00");
        data("CDMAHOME previous with no pointer reads last", tx("00B2000305"), "160000FF00");
        sw("CDMAHOME next past last -> 6A83", tx("00B2000205"), 0x6A83);
        data("CDMAHOME absolute read leaves pointer", tx("00B2010405"), "0000000000");
        sw("CDMAHOME next still past last -> 6A83", tx("00B2000205"), 0x6A83);
        data("SEARCH RECORD finds rec 20", tx("00A20004021600"), "14");
        sw("SEARCH RECORD no match -> 6A83", tx("00A2000402FFFF"), 0x6A83);
        data("SEARCH RECORD backward from 20 finds 20", tx("00A21405021600"), "14");
        sw("SEARCH RECORD backward from 19 misses -> 6A83", tx("00A21305021600"), 0x6A83);
        sw("SEARCH RECORD enhanced -> 6A81", tx("00A200060400001600"), 0x6A81);

        // Store ESN_MEID_ME.
        sw("store ESN wrong length nibble -> 6A80", tx("80DE0000080702343 4F0000000".replace(" ", "")), 0x6A80);
        data("store ESN first time -> changed", tx("80DE000008040234 34F0000000".replace(" ", "")), "01");
        data("store ESN again -> unchanged", tx("80DE000008040234 34F0000000".replace(" ", "")), "00");
        sw("select ESN_ME", tx("00A4000C026F38"), 0x9000);
        data("ESN_ME content", tx("00B0000008"), "0402343 4F0000000".replace(" ", ""));
        sw("select USGIND", tx("00A4000C026F42"), 0x9000);
        sw("set USGIND b1|b2", tx("00D600000103"), 0x9000);
        data("store MEID reports UIM_ID + SF_EUIMID usage", tx("80DE010008077856341200 00A0".replace(" ", "")), "31");
        data("store ESN reports UIM_ID usage only", tx("80DE000008040234 34F0000000".replace(" ", "")), "11");

        // EF_DIR shadow at MF, ICCID NEVER.
        sw("select MF with empty data", tx("00A4000C"), 0x9000);
        sw("select 6F22 while at MF -> 6A82", tx("00A4000C026F22"), 0x6A82);
        sw("CLA A0 accepted for SELECT", tx("A0A4000C023F00"), 0x9000);
        sw("select EF_DIR", tx("00A4000C022F00"), 0x9000);
        data("EF_DIR record 1", tx("00B2010426"), "6118 4F10" + hex(AID_BYTES) + "5004 4353494D" + "FF".repeat(12));
        sw("select ICCID", tx("00A4000C022FE2"), 0x9000);
        sw("update ICCID -> 6982", tx("00D600000A981032547698103254F5"), 0x6982);
        sw("preperso ICCID", tx("80F42FE20A981032547698103254F5"), 0x9000);
        data("read ICCID", tx("00B000000A"), "981032547698103254F5");
        sw("select by path 3F00 7FFF 6F22", tx("00A4080C063F007FFF6F22"), 0x9000);
        data("read IMSI_M via path select", tx("00B000000A"), "00A1B2C3D4E5F6A7B8C9");
        sw("select 7FFF", tx("00A4000C027FFF"), 0x9000);
        sw("select parent of ADF (MF)", tx("00A4030C00"), 0x9000);
        sw("STATUS at MF", tx("80F2000000"), 0x9000);
        sw("STATUS on CLA 00 is accepted too", tx("00F2000000"), 0x9000);
        sw("select ADF partial AID", tx("00A404000 7A0000003431002".replace(" ", "")), 0x9000);
        r = tx("80F2000100");
        data("STATUS P2=01 returns DF name", r, "8410" + hex(AID_BYTES));
        // Re-selecting the application starts a new session: PIN state is gone.
        sw("read IMSI_M after re-select -> 6982", tx("00B084000A"), 0x6982);
        sw("verify ADM1 again", tx("0020000A080000000000000000"), 0x9000);
        sw("verify PIN1 again", tx("002000010831323334FFFFFFFF"), 0x9000);

        // Deactivate / activate.
        sw("select IMSI_T", tx("00A4000C026F23"), 0x9000);
        sw("DEACTIVATE (ADM)", tx("00040000"), 0x9000);
        r = tx("00A40004026F23");
        sw("select deactivated EF -> 6283", r, 0x6283);
        check("deactivated FCP shows 8A 04", contains(r.getData(), "8A0104"), hex(r.getData()));
        sw("read deactivated -> 6986", tx("00B000000A"), 0x6986);
        sw("DEACTIVATE again -> 6985", tx("00040000"), 0x6985);
        sw("ACTIVATE (PIN1)", tx("00440000"), 0x9000);
        data("read IMSI_T after activate", tx("00B000000A"), "00000000000000000000");
        sw("DEACTIVATE by path 7FFF 6F23", tx("00040800047FFF6F23"), 0x9000);
        sw("ACTIVATE by FID 6F23", tx("00440000026F23"), 0x9000);
        data("read IMSI_T after path (de)activate", tx("00B000000A"), "00000000000000000000");
        sw("select RUIMID", tx("00A4000C026F31"), 0x9000);
        sw("DEACTIVATE RUIMID -> 6982 (NEVER)", tx("00040000"), 0x6982);

        // Defaults: ECC and LI are FF, TMSI code bytes FF, SMS record 00 FF...
        sw("select ECC", tx("00A4000C026F47"), 0x9000);
        data("ECC default FF", tx("00B000000F"), "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");
        sw("select TMSI", tx("00A4000C026F24"), 0x9000);
        data("TMSI default", tx("00B0000010"), "000000000000000000FFFFFFFF000000");
        sw("select SMS", tx("00A4000C026F3C"), 0x9000);
        r = tx("00B2010404");
        data("SMS record default 00 FF FF FF", r, "00FFFFFF");
        sw("select MMSN", tx("00A4000C026F65"), 0x9000);
        data("MMSN record default 00 00 00 FF", tx("00B2010404"), "000000FF");
        sw("select EST", tx("00A4000C026F75"), 0x9000);
        sw("update EST without PIN2 -> 6982", tx("00D600000100"), 0x6982);
        sw("verify PIN2", tx("00200081083232323 2FFFFFFFF".replace(" ", "")), 0x9000);
        sw("update EST with PIN2", tx("00D600000101"), 0x9000);

        // PIN disable / enable.
        sw("CHANGE PIN1", tx("00240001103132333 4FFFFFFFF3536373 8FFFFFFFF".replace(" ", "")), 0x9000);
        sw("DISABLE PIN1 with old value -> 63C2", tx("00260001083132333 4FFFFFFFF".replace(" ", "")), 0x63C2);
        sw("DISABLE PIN1", tx("00260001083536373 8FFFFFFFF".replace(" ", "")), 0x9000);
        sw("VERIFY status query with disabled PIN1 -> 63C3", tx("0020000100"), 0x63C3);
        sw("VERIFY disabled PIN1 with data -> 6985", tx("002000010835363738FFFFFFFF"), 0x6985);
        sw("DISABLE again -> 6985", tx("00260001083536373 8FFFFFFFF".replace(" ", "")), 0x6985);
        sw("CHANGE disabled PIN1 -> 6985", tx("00240001103536373 8FFFFFFFF3132333 4FFFFFFFF".replace(" ", "")), 0x6985);
        r = tx("00A4000402 7FFF".replace(" ", ""));
        check("ADF FCP PS_DO shows PIN1 disabled", contains(r.getData(), "C60C900160"), hex(r.getData()));
        sw("ENABLE PIN1", tx("00280001083536373 8FFFFFFFF".replace(" ", "")), 0x9000);
        sw("UNBLOCK status query -> 63CA", tx("002C000100"), 0x63CA);
        sw("DISABLE PIN1 again", tx("00260001083536373 8FFFFFFFF".replace(" ", "")), 0x9000);
        sw("UNBLOCK PIN1 with new PIN", tx("002C000110313233343536373 83132333 4FFFFFFFF".replace(" ", "")), 0x9000);
        r = tx("00A4000402 7FFF".replace(" ", ""));
        check("UNBLOCK re-enabled PIN1 (PS_DO)", contains(r.getData(), "C60C9001E0"), hex(r.getData()));
        sw("verify PIN1 restored", tx("00200001083132333 4FFFFFFFF".replace(" ", "")), 0x9000);
        r = tx("80FE000000");
        check("platform info is 39 bytes", r.getSW() == 0x9000 && r.getData().length == 39, String.format("SW=%04X len=%d %s", r.getSW(), r.getData().length, hex(r.getData())));
        check("platform info: direct and copy checksums agree", r.getSW() == 0x9000
              && hex(r.getData()).substring(32, 36).equals(hex(r.getData()).substring(36, 40))
              && hex(r.getData()).substring(40, 44).equals(hex(r.getData()).substring(44, 48)), hex(r.getData()));

        // CAVE authentication, S.S0053 §3.1 test vectors: A-key
        // C442F56BE9E17158, ESN D75A96EC, SSD from RANDSSD 4D18EEAA05895C.
        // IMSI_M bytes 4-6 hold IMSI_M_S1 (MIN1) little-endian, so MIN1
        // 792971 is stored 71 29 79.
        sw("select EF_IMSI_M for CAVE", tx("00A4000C026F22"), 0x9000);
        sw("set IMSI_M (MIN1 792971)", tx("00D600000A00000071297900800000"), 0x9000);
        // Load the known SSD directly (SSD_A || SSD_B).
        sw("load SSD (ADM)", tx("80F6010010CC3812949F4DCD0D31050234580E63B4"), 0x9000);

        // Run CAVE, global challenge, no dialed digits -> AUTH_DATA = MIN1.
        // RAND=34A2B05F, Process_Control save-registers on (0x10).
        r = tx("00880080110034A2B05F0000000010EC965AD700000000");
        data("Run CAVE vector 1 AUTHR", r, "0366F6");
        // Generate Key/VPM: CMEA key + full VPM (octets 00..40).
        r = tx("808E000002004000");
        data("Generate Key/VPM vector 1",
             r,
             "A07B1CD102756914"
             + "189394824A1A2F99A539F95B4D22D57CEE32AC216B260D36A7C96388578CB957"
             + "E2D6CA1D77B61FD5C71A73A417B2121E953470E39BCA3FD050BE4FD64780CCB8DF");
        // Run CAVE vectors 2 and 3 (save off).
        data("Run CAVE vector 2 AUTHR",
             tx("00880080110053 75DF990000000000EC965AD700000000".replace(" ", "")), "00255A");
        data("Run CAVE vector 3 AUTHR",
             tx("00880080110 06C0002580000000000EC965AD700000000".replace(" ", "")), "008A8A");
        // Save-off Run CAVE does not disturb the registers saved by vector
        // 1, so Generate Key/VPM still works. A subset returns key + range.
        r = tx("808E0000020005" + "00");
        check("Generate Key/VPM subset 00..05 is key + 6 octets",
             r.getSW() == 0x9000 && r.getData().length == 14, String.format("SW=%04X len=%d", r.getSW(), r.getData().length));
        // VPM blocks are generated only up to the last requested octet, so
        // key-only and tail ranges must still match the full vector.
        data("Generate Key/VPM key only", tx("808E000002FFFF00"), "A07B1CD102756914");
        data("Generate Key/VPM tail 3C..40", tx("808E0000023C4000"),
             "A07B1CD102756914" + "4780CCB8DF");
        // The saved registers survive the ME re-selecting the CSIM between
        // Run CAVE and Generate Key/VPM (C.S0023-D §4.4.4.2).
        sw("select MF before CSIM re-select", tx("00A40004023F00"), 0x9000);
        sw("re-select CSIM ADF", tx(concat("00A40404", AID_BYTES, "00")), 0x9000);
        sw("verify PIN1 after re-select", tx("00200001083132333 4FFFFFFFF".replace(" ", "")), 0x9000);
        data("Generate Key/VPM after CSIM re-select", tx("808E000002FFFF00"), "A07B1CD102756914");
        sw("verify ADM1 after re-select", tx("0020000A080000000000000000"), 0x9000);

        // Unique challenge: RANDU in octets 3-5, RANDTYPE=01.
        r = tx("0088008011" + "01" + "00" + "34A2B0" + "00" + "000000" + "00" + "EC965AD7000000" + "00");
        check("Run CAVE unique challenge returns AUTHU", r.getSW() == 0x9000 && r.getData().length == 3, String.format("SW=%04X len=%d", r.getSW(), r.getData().length));

        // A-key checksum (S.S0053 §2.2.1): load the A-key, then checksum
        // over ESN D75A96EC must be the entered check digits 086500.
        sw("load A-key (ADM)", tx("80F6000008C442F56BE9E17158"), 0x9000);
        data("A-key checksum digits (086500)", tx("80F8000004D75A96EC00"), "303836353030");

        // SSD generation + update + confirm, end to end.
        caveDeterministicRegression(meta);
        caveRamTableReset(meta);
        caveSsdGeneration(meta);

        // OTASP A-key Diffie-Hellman end to end (C.S0016-C §5, p_rev 2).
        caveOtaspDh(meta);
        // p_rev 3: A-key + 3G Root Key via the S.S0055 f0 kernel.
        caveOtaspDh3(meta);

        // OTAPA session + SPASM (C.S0016-C §3.3.7).
        caveOtapa(meta);

        // OTASP Validate: Verify / Change SPC against EF_SPC (via PREPERSO).
        sw("otasp: preperso EF_SPC = 123456", tx("80F46F3303" + "123456"), 0x9000);
        data("otasp: Config while SP locked -> 0A", tx("80540000010100"), "01000A");
        data("otasp: Download while SP locked -> 0A",
             tx("8056000005" + "0103" + "512345" + "02"), "010A");
        data("otasp: Commit while SP locked -> 0A", tx("80CC000001"), "0A");
        data("otasp: Change SPC while SP locked -> 0A", tx("80CE000005" + "0103654321"), "010A");
        data("otasp: Verify wrong SPC -> invalid SPC", tx("80CE000005" + "0003999999"), "000B");
        data("otasp: Verify SPC -> accept", tx("80CE000005" + "0003123456"), "0000");
        data("otasp: Change SPC -> accept", tx("80CE000005" + "0103654321"), "0100");
        data("otasp: Verify new SPC -> accept", tx("80CE000005" + "0003654321"), "0000");

        // Generic Download / Config + Commit with real NAM blocks. The
        // Mobile Directory Number block (ID 01) holds N_DIGITS and 4-bit
        // digits. Digits 1 2 3 4 5 pack to 51 23 45 and commit to EF_MDN.
        data("otasp: Download MDN block (digits 12345) -> accept",
             tx("8056000005" + "0103" + "512345" + "02"), "0100");
        sw("otasp: Commit applies staged NAM blocks", tx("80CC000001"), 0x9000);
        sw("otasp: select EF_MDN", tx("00A4000C026F44"), 0x9000);
        data("otasp: EF_MDN holds decoded digits", tx("00B2010400"), "052143F5FFFFFFFFFF0000");
        data("otasp: Config rebuilds MDN block from EF", tx("80540000010100"), "010300512345");

        // SSPR PRL segment download writes straight into EF_EPRL at offset.
        data("otasp: SSPR PRL segment @0x10 -> accept",
             tx("8056000108" + "0006" + "0010AABBCCDD" + "02"), "0000");
        sw("otasp: Commit (activates PRL)", tx("80CC000001"), 0x9000);
        sw("otasp: select EF_EPRL", tx("00A4000C026F5A"), 0x9000);
        data("otasp: EF_EPRL segment written", tx("00B0001004"), "AABBCCDD");

        // Configuration Request for non-NAM blocks, built from the EFs.
        sw("otasp: select EF_3GPDOPM", tx("00A4000C026F49"), 0x9000);
        sw("otasp: set EF_3GPDOPM", tx("00D600000102"), 0x9000);
        data("otasp: 3GPD Config from EF (P2=03)", tx("80540003010000"), "00010002");
        sw("otasp: select EF_MAXPRL", tx("00A4000C026F45"), 0x9000);
        sw("otasp: set EF_MAXPRL", tx("00D600000401020304"), 0x9000);
        data("otasp: SSPR Config PRL dimensions (P2=01)", tx("80540001010000"), "00040001020304");
        // The other OTASP families (PUZL P2=02, System Tag P2=05) download
        // a segment to their EF and a Config Request reads it back.
        data("otasp: PUZL Download segment", tx("805600020800060000AABBCCDD02"), "0000");
        r = tx("80540002010000");
        check("otasp: PUZL Config from EF", r.getSW() == 0x9000 && hex(r.getData()).startsWith("002000AABBCCDD"), hex(r.getData()));
        data("otasp: System Tag Download segment", tx("805600050800060000EEFF112202"), "0000");
        r = tx("80540005010000");
        check("otasp: System Tag Config from EF", r.getSW() == 0x9000 && hex(r.getData()).startsWith("002000EEFF1122"), hex(r.getData()));

        // IMSI_T NAM block (ID 03) round-trips EF_IMSI_T through Config/Download.
        sw("otasp: select EF_IMSI_T", tx("00A4000C026F23"), 0x9000);
        sw("otasp: set EF_IMSI_T", tx("00D600000A00000071297900800000"), 0x9000);
        ResponseAPDU it = tx("80540000010300");
        check("otasp: Config IMSI_T block from EF", it.getSW() == 0x9000 && it.getData().length >= 3,
             String.format("SW=%04X", it.getSW()));
        byte itLen = it.getData()[1];
        byte[] itBlock = slice(it.getData(), 3, itLen & 0xFF);
        byte[] itDl = cat(hex("80560000"), new byte[] { (byte) (2 + itBlock.length), 0x03, itLen }, itBlock, hex("02"));
        sw("otasp: Download IMSI_T block", tx(itDl), 0x9000);
        sw("otasp: Commit IMSI_T", tx("80CC000001"), 0x9000);
        sw("otasp: reselect EF_IMSI_T", tx("00A4000C026F23"), 0x9000);
        data("otasp: EF_IMSI_T round-trips", tx("00B000000A"), "00000071297900800000");

        // CDMA NAM Download block -> EF_IMSI_M, validated end to end through
        // the CAVE AUTHR vector (block IMSI_M_S low 24 bits = MIN1 792971).
        caveNamImsi(meta);

        // FRESH put/get (Crypto-Sync).
        sw("otasp: FRESH put", tx("804C000002ABCD"), 0x9000);
        data("otasp: FRESH get", tx("804C010002"), "ABCD");

        // Secure Mode start/stop and ESP parameter-block encryption.
        data("otasp: Secure Mode start", tx("804A000008" + "1122334455667788" + "01"), "00");
        data("otasp: Secure Mode stop", tx("804A010001"), "00");
        // ESP keystream vector (S.S0055-A Exhibit 4-1).
        if (org.onexbts.csim.Variant.DEBUG) data("ss0055 ESP maskbits vector",
             tx("80E603002A" + "54657374206b65792031323862697473" + "08" + "0000000000000001"
                + "10" + "00000000000000000000000000000000" + "00"),
             "ad2308ad191d9371d950f4d7a3a1480c");
        // End-to-end Secure Mode: Config encrypts, Download decrypts.
        caveSecureMode(meta);
        // Service Key Generation -> WLAN root key -> WLAN AKA.
        caveServiceKey(meta);

        // Packet-data IP authentication (C.S0023-D §4.8.1, MD5). Expected
        // values computed independently with MD5.
        sw("ip: load SIP CHAP SS", tx("80F0000008" + "0011223344556677"), 0x9000);
        data("ip: CHAP response", tx("8080000006" + "1A00AABBCCDD" + "10"),
             "491223C8729F5E8A9E8BFF9FD961996E");
        sw("ip: load HRPD SS", tx("80F0030004" + "DEADBEEF"), 0x9000);
        data("ip: HRPD CHAP response", tx("8080040006" + "050102030405" + "10"),
             "E10C7CBF0F52CD63C1ADB762934DEFB3");
        sw("ip: load MN-HA SS", tx("80F0010008" + "A0A1A2A3A4A5A6A7"), 0x9000);
        data("ip: MN-HA single block", tx("8080010206" + "001122334455" + "10"),
             "50E3634264D20AAD820A6E06488A6BD8");
        // Same MN-HA result over chained first + last blocks.
        sw("ip: MN-HA first block", tx("8080010003" + "001122"), 0x9000);
        data("ip: MN-HA last block", tx("8080010303" + "334455" + "10"),
             "50E3634264D20AAD820A6E06488A6BD8");
        sw("ip: load MN-AAA SS", tx("80F0020004" + "B0B1B2B3"), 0x9000);
        // MN-AAA before MIP-RRQ Hash -> 9834.
        sw("ip: MN-AAA before MIP-RRQ -> 9834", tx("8080030005" + "00C0C1C2C3" + "10"), 0x9834);
        sw("ip: MIP-RRQ Hash single block", tx("8080020208" + "DEADC0DE11223344"), 0x9000);
        data("ip: MN-AAA authenticator", tx("8080030005" + "00C0C1C2C3" + "10"),
             "023B0C8BC2F526D7B675E0D3886B5F02");

        // 3G AKA with the S.S0055 f0-f5/f1*/f11 kernel (Exhibit 4-3).
        sw("aka: before key load -> 6985", tx("0088008121" + AKA_RAND + "10" + AKA_AUTN + "00"), 0x6985);
        sw("aka: load K", tx("80EA000010" + AKA_K), 0x9000);
        data("aka: authenticate -> CK/IK/RES",
             tx("0088008121" + AKA_RAND + "10" + AKA_AUTN + "00"),
             "00" + AKA_CK + AKA_IK + "10" + AKA_RES);
        // Replay the same AUTN: SQN not greater -> resync. AUTS first
        // 6 bytes = SQN_MS(000000000001) ^ AK*(f5* b2d637365cea).
        ResponseAPDU rs = tx("0088008121" + AKA_RAND + "10" + AKA_AUTN + "00");
        check("aka: replay (stale SQN) -> resync AUTS",
             rs.getSW() == 0x9000 && rs.getData().length == 15 && rs.getData()[0] == 1
             && hex(slice(rs.getData(), 1, 6)).equalsIgnoreCase("b2d637365ceb"),
             "SW=" + String.format("%04X", rs.getSW()) + " data=" + hex(rs.getData()));
        sw("aka: MAC failure -> 9804",
             tx("0088008121" + AKA_RAND + "10" + "594cc7c17c0700016abdc4da73c61b8e" + "00"), 0x9804);

        // UMAC uses the f11 UAK. CONFIRM_KEYS persists CK/IK.
        data("aka: UMAC -> success tag 1 + UMAC", tx("805E00000411223344" + "00"), "01617fa2a3");
        sw("aka: CONFIRM_KEYS", tx("805C0000"), 0x9000);
        sw("aka: select EF_3GCIK", tx("00A4000C026F6B"), 0x9000);
        data("aka: EF_3GCIK holds CK||IK", tx("00B0000020"), AKA_CK + AKA_IK);

        // WLAN AKA (P2=82) uses the WLAN root key, on a fresh card so the
        // shared SQN_MS has not already consumed this sequence number.
        CardSimulator w = freshCaveSim(meta);
        swOn("aka: WLAN before key -> 6985", w, "0088008221" + AKA_RAND + "10" + AKA_AUTN + "00", 0x6985);
        swOn("aka: load WLAN K", w, "80EA010010" + AKA_K, 0x9000);
        ResponseAPDU wa = txOn(w, "0088008221" + AKA_RAND + "10" + AKA_AUTN + "00");
        check("aka: WLAN authenticate -> CK/IK/RES",
             wa.getSW() == 0x9000 && hex(wa.getData()).equalsIgnoreCase("00" + AKA_CK + AKA_IK + "10" + AKA_RES),
             "SW=" + String.format("%04X", wa.getSW()) + " data=" + hex(wa.getData()));
        sw("Confirm SSD with no candidate -> 9834", tx("808200010311 2233".replace(" ", "")), 0x9834);
        sw("CLA 60 -> 6E00", tx("60A4000C023F00"), 0x6E00);

        // S.S0055 AKA kernel vectors (Exhibit 4-3).
        if (!org.onexbts.csim.Variant.DEBUG) {
            sw("hardened: crypto self-test -> 6D00", tx("80E6000018"+"b0abb99d6ac6a74eb98eb6c2dab1a551"+"0000000000000000"+"00"), 0x6D00);
        } else {
        data("ss0055 f0", tx("80E6000018"+"b0abb99d6ac6a74eb98eb6c2dab1a551"+"0000000000000000"+"00"), "4b052b20e2a06c8f");
        data("ss0055 f1", tx("80E6010028"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"000000000001"+"0001"+"00"), "6abdc4da73c61b8d");
        data("ss0055 f3", tx("80E6020020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "6efdd832f6ffd4dca84a5496fa6e2993");
        data("ss0055 f1*", tx("80E6040028"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"000000000001"+"0001"+"00"), "b017359d5da881a0");
        data("ss0055 f2", tx("80E6050020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "d82e282adc13c0f1686566339bf27eb6");
        data("ss0055 f4", tx("80E6060020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "c1436525fa607f1792fca89fb2a7bc4a");
        data("ss0055 f5", tx("80E6070020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "594cc7c17c06");
        data("ss0055 f5*", tx("80E6080020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "b2d637365cea");
        data("ss0055 f11", tx("80E6090020"+"ad1b5a159be86b2ca66c7ae40bba9b9d"+"4b052b20e2a06c8ff700da512b4e111e"+"00"), "5501c020869b8fef7a33bb12a0d02e63");
        }

        hardening(meta);
        cyclicPersistence(meta);
        ssdCandidatePersistence(meta);

        System.out.println(failures == 0 ? "ALL PASS" : failures + " FAILURES");
        System.exit(failures == 0 ? 0 : 1);
    }

    static void caveDeterministicRegression(byte[] meta) throws Exception {
        CardSimulator card = freshCaveSim(meta);
        java.util.Random random = new java.util.Random(0xCAFE);
        java.security.MessageDigest digest = java.security.MessageDigest.getInstance("SHA-256");
        for (int i = 0; i < 512; i++) {
            byte[] secret = new byte[16];
            byte[] input = new byte[21];
            random.nextBytes(input);
            System.arraycopy(input, 5, secret, 0, secret.length);
            ResponseAPDU r = txOn(card, "80F6010010" + hex(secret));
            if (r.getSW() != 0x9000) throw new Exception("regression SSD load failed");
            byte[] auth = new byte[23];
            random.nextBytes(auth);
            auth[0] = 0;
            auth[1] = (byte) 0x88;
            auth[2] = 0;
            auth[3] = (byte) 0x80;
            auth[4] = 17;
            auth[5] = 0;
            auth[10] = 24;
            auth[14] = 16;
            auth[19] = 0;
            auth[20] = 0;
            auth[21] = 0;
            auth[22] = 0;
            r = card.transmitCommand(new CommandAPDU(auth));
            if (r.getSW() != 0x9000) throw new Exception("regression authentication failed");
            digest.update(r.getData());
            r = txOn(card, "808E000002004000");
            if (r.getSW() != 0x9000) throw new Exception("regression key/VPM failed");
            digest.update(r.getData());
        }
        check("CAVE deterministic signature and full-VPM regression",
              hex(digest.digest()).equals("E625C8541EF00D601A55D9271C4835CE86D209DBE9677128F7E0349C5B9FEBB8"),
              "digest mismatch");
    }

    static void caveRamTableReset(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        swOn("RAM table: load SSD", s, "80F6010010CC3812949F4DCD0D31050234580E63B4", 0x9000);
        for (int pass = 0; pass < 2; pass++) {
            data("RAM table: AUTHR pass " + pass,
                 txOn(s, "00880080110034A2B05F0000000010EC965AD700000000"), "0366F6");
            data("RAM table: full key/VPM pass " + pass, txOn(s, "808E000002004000"),
                 "A07B1CD102756914"
                 + "189394824A1A2F99A539F95B4D22D57CEE32AC216B260D36A7C96388578CB957"
                 + "E2D6CA1D77B61FD5C71A73A417B2121E953470E39BCA3FD050BE4FD64780CCB8DF");
            if (pass == 0) {
                s.reset();
                swOn("RAM table: select after reset", s, concat("00A40404", AID_BYTES, "00"), 0x9000);
                swOn("RAM table: verify after reset", s, "002000010831323334FFFFFFFF", 0x9000);
                swOn("RAM table: saved registers cleared", s, "808E000002004000", 0x9834);
            }
        }
    }

    static void cyclicPersistence(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        s.reset();
        swOn("ring: select CSIM", s, concat("00A40404", AID_BYTES, "00"), 0x9000);
        swOn("ring: verify PIN", s, "002000010831323334FFFFFFFF", 0x9000);
        swOn("ring: select COUNT", s, "00A4000C026F21", 0x9000);
        for (int value = 1; value <= 25; value++) {
            data("ring: increase " + value, txOn(s, "0032000002000100"), String.format("%04X0001", value));
            for (int record = 1; record <= 10; record++) {
                data("ring: history " + value + "/" + record,
                     txOn(s, String.format("00B2%02X0402", record)),
                     String.format("%04X", Math.max(0, value - record + 1)));
            }
        }
        swOn("ring: overflow", s, "0032000002FFFF", 0x9850);
        swOn("ring: short record", s, "00DC00030100", 0x6700);
        s.reset();
        swOn("ring: reselect after reset", s, concat("00A40404", AID_BYTES, "00"), 0x9000);
        swOn("ring: reverify after reset", s, "002000010831323334FFFFFFFF", 0x9000);
        swOn("ring: reselect COUNT", s, "00A4000C026F21", 0x9000);
        for (int record = 1; record <= 10; record++) {
            data("ring: persisted history " + record,
                 txOn(s, String.format("00B2%02X0402", record)), String.format("%04X", 26 - record));
        }
        data("ring: increase after reset", txOn(s, "0032000002000100"), "001A0001");
        swOn("ring: same value still advances history", s, "00DC000302001A", 0x9000);
        data("ring: duplicate newest", txOn(s, "00B2010402"), "001A");
        data("ring: duplicate previous", txOn(s, "00B2020402"), "001A");
    }

    static void ssdCandidatePersistence(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        swOn("candidate: load A-key", s, "80F6000008C442F56BE9E17158", 0x9000);
        swOn("candidate: load active SSD", s, "80F6010010CC3812949F4DCD0D31050234580E63B4", 0x9000);
        swOn("candidate: challenge", s, "808A0000041234567804", 0x9000);
        swOn("candidate: update deferred", s, "808200000F1122334455667704EC965AD7000000", 0x9000);
        s.reset();
        swOn("candidate: select after reset", s, concat("00A40404", AID_BYTES, "00"), 0x9000);
        swOn("candidate: verify after reset", s, "002000010831323334FFFFFFFF", 0x9000);
        swOn("candidate: confirm after reset rejected", s, "8082000103000000", 0x9834);
        data("candidate: active SSD survives reset", txOn(s, "00880080110034A2B05F0000000000EC965AD700000000"), "0366F6");
    }

    static void caveSsdGeneration(byte[] meta) throws Exception {
        CardSimulator main = freshCaveSim(meta);
        swOn("ssd: load A-key", main, "80F6000008C442F56BE9E17158", 0x9000);
        ResponseAPDU bs = txOn(main, "808A00000400000000" + "04");
        check("ssd: Base Station Challenge returns 4-byte RANDBS",
             bs.getSW() == 0x9000 && bs.getData().length == 4,
             String.format("SW=%04X len=%d", bs.getSW(), bs.getData().length));
        byte[] randbs = bs.getData();
        // Update SSD (P1=00 P2=00): RANDSSD(7) PC(00) ESN(7), Lc=15.
        swOn("ssd: Update SSD", main, "808200000F" + "4D18EEAA05895C" + "00" + "EC965AD7000000", 0x9000);

        // Reference card: known SSD, then Run CAVE over the same RANDBS.
        CardSimulator ref = freshCaveSim(meta);
        swOn("ssd: ref load SSD", ref, "80F6010010CC3812949F4DCD0D31050234580E63B4", 0x9000);
        byte[] runcave = cat(hex("0088008011"), new byte[] { 0x00 }, randbs,
                             hex("0000000000"), hex("EC965AD7000000"), hex("00"));
        ResponseAPDU refr = txOn(ref, runcave);
        check("ssd: ref AUTHBS computed", refr.getSW() == 0x9000 && refr.getData().length == 3,
             String.format("SW=%04X", refr.getSW()));
        byte[] authbs = refr.getData();

        // Confirm SSD (P1=00 P2=01) with the expected AUTHBS -> commit.
        byte[] confirm = cat(hex("8082000103"), authbs);
        swOn("ssd: Confirm SSD commits (generated SSD == known)", main, confirm, 0x9000);
        // Active SSD is now the generated SSD: Run CAVE must match vector 1.
        ResponseAPDU fin = txOn(main, "00880080110034A2B05F0000000000EC965AD700000000");
        data("ssd: post-commit Run CAVE matches AUTHR vector 1", fin, "0366F6");
    }

    /**
     * OTASP A-key Diffie-Hellman. The card picks a secret X and returns
     * MS_RESULT = G^X mod P. With BS_RESULT = G^Y mod P the card derives
     * A-key = low 64 bits of (BS_RESULT^X mod P) = low 64 bits of
     * (MS_RESULT^Y mod P), which the harness computes with BigInteger.
     * After Commit the A-key is checked against a reference card loaded
     * with that value, comparing on-card A-key checksums over two ESNs.
     */
    static void caveOtaspDh(byte[] meta) throws Exception {
        String Phex = "C03987108976E334E2817EFDAE8492171D53434BB88139B9AE270DA702F06B90"
                    + "F143262FDC5C0EED8DA0365BF89897B9405CACEC877409A977D21E02FF01CF99";
        BigInteger P = new BigInteger(1, hex(Phex));
        BigInteger Y = BigInteger.valueOf(3);
        BigInteger BS = BigInteger.valueOf(2).modPow(Y, P);   // G^Y mod P

        CardSimulator dh = freshCaveSim(meta);
        // MS Key Request: RANDSeed(20) P_REV(02) P_len(40) G_len(01) P G(02).
        String msData = "0000000000000000000000000000000000000000" + "02" + "40" + "01" + Phex + "02";
        ResponseAPDU r = txOn(dh, "80500000" + String.format("%02X", msData.length() / 2) + msData + "01");
        check("otasp: MS Key Request -> result 00", r.getSW() == 0x9000 && r.getData().length == 1 && r.getData()[0] == 0,
             String.format("SW=%04X data=%s", r.getSW(), hex(r.getData())));
        // Key Generation Request with BS_RESULT = G^Y mod P.
        byte[] bs = unsignedBytes(BS);
        byte[] kg = cat(hex("80500001"), new byte[] { (byte) (bs.length + 1), (byte) bs.length }, bs, hex("00"));
        r = txOn(dh, kg);
        check("otasp: Key Generation Request returns MS_RESULT",
             r.getSW() == 0x9000 && r.getData().length == 66 && r.getData()[0] == 0 && (r.getData()[1] & 0xFF) == 64,
             String.format("SW=%04X len=%d", r.getSW(), r.getData().length));
        byte[] msResult = slice(r.getData(), 2, 64);
        BigInteger ms = new BigInteger(1, msResult);
        byte[] shared = to64(ms.modPow(Y, P));
        byte[] expectedAkey = slice(shared, 56, 8);          // low 64 bits
        swOn("otasp: Commit", dh, "80CC000001", 0x9000);

        // Reference card loaded with the expected A-key.
        CardSimulator ref = freshCaveSim(meta);
        swOn("otasp: ref load A-key", ref, cat(hex("80F6000008"), expectedAkey), 0x9000);
        // Compare A-key checksums over two ESNs (36-bit collision margin).
        String[] esns = { "D75A96EC", "01020304" };
        for (String esn : esns) {
            ResponseAPDU a = txOn(dh, "80F8000004" + esn + "00");
            ResponseAPDU b = txOn(ref, "80F8000004" + esn + "00");
            check("otasp: committed A-key matches DH result (ESN " + esn + ")",
                 a.getSW() == 0x9000 && b.getSW() == 0x9000 && hex(a.getData()).equals(hex(b.getData())),
                 "dh=" + hex(a.getData()) + " ref=" + hex(b.getData()));
        }
    }

    static final String OTASP_P_HEX =
        "C03987108976E334E2817EFDAE8492171D53434BB88139B9AE270DA702F06B90"
      + "F143262FDC5C0EED8DA0365BF89897B9405CACEC877409A977D21E02FF01CF99";

    /**
     * OTASP A-key DH with A_KEY_P_REV 3: the DH result's low 128 bits seed
     * the S.S0055 f0 kernel (counter 0) for the A-key. The harness derives
     * RAND_SEED with BigInteger, obtains the expected A-key from the card's
     * own (vector-validated) f0, and checks the committed A-key against it.
     */
    static void caveOtaspDh3(byte[] meta) throws Exception {
        BigInteger P = new BigInteger(1, hex(OTASP_P_HEX));
        BigInteger Y = BigInteger.valueOf(3);
        BigInteger BS = BigInteger.valueOf(2).modPow(Y, P);
        CardSimulator dh = freshCaveSim(meta);
        String msData = "0000000000000000000000000000000000000000" + "03" + "40" + "01" + OTASP_P_HEX + "02";
        swOn("otasp3: MS Key Request (p_rev 3)", dh, "80500000" + String.format("%02X", msData.length() / 2) + msData + "01", 0x9000);
        byte[] bs = unsignedBytes(BS);
        byte[] kg = cat(hex("80500001"), new byte[] { (byte) (bs.length + 1), (byte) bs.length }, bs, hex("00"));
        ResponseAPDU r = txOn(dh, kg);
        check("otasp3: Key Generation Request", r.getSW() == 0x9000 && r.getData().length == 66, String.format("SW=%04X", r.getSW()));
        byte[] msResult = slice(r.getData(), 2, 64);
        BigInteger shared = new BigInteger(1, msResult).modPow(Y, P);
        byte[] seed = slice(to64(shared), 48, 16);   // RAND_SEED = low 128 bits
        // Expected A-key = f0(RAND_SEED, counter 0) from the card's kernel,
        // which only the debug self-test command exposes.
        if (!org.onexbts.csim.Variant.DEBUG) {
            swOn("otasp3: Commit", dh, "80CC000001", 0x9000);
            return;
        }
        ResponseAPDU f0 = txOn(dh, cat(hex("80E6000018"), seed, hex("000000000000000000")));
        byte[] expectedAkey = slice(f0.getData(), 0, 8);
        swOn("otasp3: Commit", dh, "80CC000001", 0x9000);
        CardSimulator ref = freshCaveSim(meta);
        swOn("otasp3: ref load A-key", ref, cat(hex("80F6000008"), expectedAkey), 0x9000);
        for (String esn : new String[] { "D75A96EC", "01020304" }) {
            ResponseAPDU a = txOn(dh, "80F8000004" + esn + "00");
            ResponseAPDU b = txOn(ref, "80F8000004" + esn + "00");
            check("otasp3: committed A-key matches f0 derivation (ESN " + esn + ")",
                 a.getSW() == 0x9000 && hex(a.getData()).equals(hex(b.getData())),
                 "dh=" + hex(a.getData()) + " ref=" + hex(b.getData()));
        }
    }

    /**
     * OTAPA session and SPASM. With A_KEY = 0 the SPASM input SSD_A ^
     * A_KEY equals SSD_A, so AUTH_OTAPA over RAND_OTAPA equals an ordinary
     * Run CAVE over the same RAND, which the harness obtains from the card
     * itself and packs into the Validate SPASM parameter block.
     */
    static void caveOtapa(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);                 // A_KEY stays 0
        swOn("otapa: load SSD", s, "80F6010010CC3812949F4DCD0D31050234580E63B4", 0x9000);
        swOn("otapa: enable SPASM (EF_OTAPASPC)", s, "80F46F340101", 0x9000);
        // Start: Start/Stop(01) RANDSeed(4) ESN(7, LSB-first) -> RAND_OTAPA.
        ResponseAPDU r = txOn(s, "80EE00000C" + "01" + "00000000" + "EC965AD7000000" + "06");
        check("otapa: Start returns RAND_OTAPA (NAM_LOCK=1)",
             r.getSW() == 0x9000 && r.getData().length == 6 && r.getData()[0] == 0 && r.getData()[1] == 1,
             String.format("SW=%04X data=%s", r.getSW(), hex(r.getData())));
        byte[] randOtapa = slice(r.getData(), 2, 4);
        // Expected AUTH_OTAPA = Run CAVE over RAND_OTAPA (A_KEY=0 so SSD_A^A_KEY=SSD_A).
        byte[] rc = cat(hex("0088008011"), new byte[] { 0x00 }, randOtapa,
                        hex("0000000000"), hex("EC965AD7000000"), hex("00"));
        ResponseAPDU a = txOn(s, rc);
        check("otapa: reference Run CAVE ok", a.getSW() == 0x9000 && a.getData().length == 3,
             String.format("SW=%04X", a.getSW()));
        byte[] spasm = packSpasm(a.getData(), 0);
        byte[] good = cat(hex("80CE000005"), new byte[] { 0x02, 0x03 }, spasm);
        data("otapa: Download while NAM locked -> 0A", txOn(s, "8056000005" + "0103" + "512345" + "02"), "010A");
        byte[] bad = cat(hex("80CE000005"), new byte[] { 0x02, 0x03 },
                         new byte[] { (byte) 0x80, 0x00, 0x00 });
        data("otapa: Validate SPASM rejects wrong AUTH_OTAPA", txOn(s, bad), "020D");
        data("otapa: Validate SPASM accepts matching AUTH_OTAPA", txOn(s, good), "0200");
        data("otapa: Download after SPASM -> accept", txOn(s, "8056000005" + "0103" + "512345" + "02"), "0100");
        swOn("otapa: Stop", s, "80EE00000C" + "00" + "00000000" + "EC965AD7000000" + "06", 0x9000);
    }

    /**
     * CDMA NAM Download block -> EF_IMSI_M. The block holds IMSI_M_S
     * with its low 24 bits = MIN1 792971, so after Commit a Run CAVE over
     * RAND1 with no dialed digits must reproduce AUTHR vector 1 (0366F6),
     * proving the block's IMSI_M_S lands in the EF bytes CAVE reads.
     */
    static void caveNamImsi(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        swOn("nam: load SSD", s, "80F6010010CC3812949F4DCD0D31050234580E63B4", 0x9000);
        // CDMA NAM block (id 02), 9 bytes, IMSI_M_S1 = 792971.
        swOn("nam: Download CDMA NAM block", s,
             "805600000B" + "0209" + "00000000F252E20000" + "02", 0x9000);
        swOn("nam: Commit", s, "80CC000001", 0x9000);
        data("nam: Run CAVE AUTHR matches vector after IMSI_M download",
             txOn(s, "00880080110034A2B05F0000000000EC965AD700000000"), "0366F6");
    }

    /**
     * End-to-end Secure Mode: with Secure Mode active, a Configuration
     * Request returns the MDN block with its param data ESP-encrypted.
     * Feeding that ciphertext straight back as a Download Request block
     * (also under Secure Mode) must decrypt to the original digits, which
     * Commit then writes to EF_MDN, proving ESP encrypt/decrypt are the
     * applied inverse keyed by the SMCK f3 derivation.
     */
    static void caveSecureMode(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        // Set EF_MDN to digits 12345 in the clear first.
        swOn("sm: seed EF_MDN", s, "8056000005" + "0103" + "512345" + "02", 0x9000);
        swOn("sm: commit seed", s, "80CC000001", 0x9000);
        // Enter Secure Mode (SSD=0 on a fresh card, so K = 0).
        swOn("sm: FRESH put", s, "804C000002ABCD", 0x9000);
        swOn("sm: Secure Mode start", s, "804A000008" + "1122334455667788" + "01", 0x9000);
        // Config MDN -> header + ESP(param data).
        ResponseAPDU cfg = txOn(s, "80540000010100");
        byte[] cd = cfg.getData();
        check("sm: Config MDN param data is encrypted", cfg.getSW() == 0x9000 && cd.length == 6
              && !hex(slice(cd, 3, 3)).equals("512345"), hex(cd));
        byte len = cd[1];
        byte[] enc = slice(cd, 3, len & 0xFF);
        // Download the ciphertext back. Secure Mode decrypts it.
        byte[] dl = cat(hex("80560000"), new byte[] { (byte) (2 + enc.length), 0x01, len }, enc, hex("02"));
        swOn("sm: Download encrypted MDN block", s, dl, 0x9000);
        swOn("sm: Commit", s, "80CC000001", 0x9000);
        swOn("sm: select EF_MDN", s, "00A4000C026F44", 0x9000);
        data("sm: EF_MDN decrypted to original digits", txOn(s, "00B2010400"), "052143F5FFFFFFFFFF0000");
    }

    /**
     * Service Key Generation (C.S0016-C §3.3.10): ITM_RAND = f0(RKP) and
     * the WLAN root key = f3(ITM_RAND, "WLAN") under the service family
     * key. The derived WLAN key must then authenticate a WLAN AKA whose
     * AUTN the harness built for that key (computed independently).
     */
    static void caveServiceKey(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        swOn("svc: Service Key Gen before K -> 6985", s, "8050000202" + "0400" + "01", 0x6985);
        swOn("svc: load 3G root key K", s, "80EA000010" + AKA_K, 0x9000);
        ResponseAPDU sk = txOn(s, "8050000202" + "0400" + "01");
        check("svc: Service Key Gen (WLAN bit) -> 00",
             sk.getSW() == 0x9000 && hex(sk.getData()).equals("00"), hex(sk.getData()));
        // WLAN AKA with the derived key and the independently built AUTN.
        ResponseAPDU wa = txOn(s, "0088008221" + AKA_RAND + "10" + "5c6e36651e510001ccd42a42c501e027" + "00");
        check("svc: WLAN AKA with derived WLAN root key",
             wa.getSW() == 0x9000 && hex(wa.getData()).equalsIgnoreCase(
                 "00" + "9fb7d3105bcb2b18482c77b59c10fe9c" + "21687d1159fb40e5ced6ba75ccc386e1"
                      + "10" + "fb239e42eeb00a50c2c32e78ff18e229"),
             "SW=" + String.format("%04X", wa.getSW()) + " data=" + hex(wa.getData()));
    }

    static byte[] packSpasm(byte[] auth3, int namLock) {
        int a0 = auth3[0] & 0xFF, a1 = auth3[1] & 0xFF, a2 = auth3[2] & 0xFF;
        byte[] out = new byte[3];
        out[0] = (byte) (0x80 | ((a0 & 0x03) << 5) | ((a1 >> 3) & 0x1F));
        out[1] = (byte) (((a1 & 0x07) << 5) | ((a2 >> 3) & 0x1F));
        out[2] = (byte) (((a2 & 0x07) << 5) | ((namLock & 1) << 4));
        return out;
    }

    static byte[] unsignedBytes(BigInteger v) {
        byte[] b = v.toByteArray();
        if (b.length > 1 && b[0] == 0) return slice(b, 1, b.length - 1);
        return b;
    }
    static byte[] to64(BigInteger v) {
        byte[] u = unsignedBytes(v);
        byte[] out = new byte[64];
        System.arraycopy(u, 0, out, 64 - u.length, u.length);
        return out;
    }

    static CardSimulator freshCaveSim(byte[] meta) throws Exception {
        CardSimulator s = new CardSimulator();
        AID aid = AIDUtil.create(AID_BYTES);
        s.installApplet(aid, org.onexbts.csim.Csim.class);
        txOn(s, concat("00A40404", AID_BYTES, "00"));
        txOn(s, "0020000A080000000000000000"); // verify ADM1
        for (int off = 0; off < meta.length; off += 100) {
            int n = Math.min(100, meta.length - off);
            boolean last = off + n >= meta.length;
            byte p1 = (byte) (((off >> 8) & 0x7F) | (last ? 0x80 : 0));
            txOn(s, concat(new byte[] { (byte) 0x80, (byte) 0xFA, p1, (byte) (off & 0xFF), (byte) n }, slice(meta, off, n), ""));
        }
        txOn(s, "0020000108313233 34FFFFFFFF".replace(" ", "")); // verify PIN1
        txOn(s, "00A4000C026F22");                               // select EF_IMSI_M
        txOn(s, "00D600000A00000071297900800000");               // IMSI_M (MIN1 792971)
        return s;
    }

    /** Reset-scoped PIN state, PUK provisioning, SPC attempt limit,
     *  INIT_META reload, empty IP secrets and the hardened PLATFORM_INFO. */
    static void hardening(byte[] meta) throws Exception {
        CardSimulator s = freshCaveSim(meta);
        swOn("hardening: INIT_META again -> 6985", s,
             concat(new byte[] { (byte) 0x80, (byte) 0xFA, (byte) 0x80, 0, 8 }, slice(meta, 0, 8), ""), 0x6985);

        swOn("hardening: CHAP with no secret -> 6985", s, "8080000006" + "1A00AABBCCDD" + "10", 0x6985);
        swOn("hardening: HRPD with no secret -> 6985", s, "8080040006" + "050102030405" + "10", 0x6985);
        swOn("hardening: MN-HA with no secret -> 6985", s, "8080010206" + "001122334455" + "10", 0x6985);

        swOn("hardening: SET PUK bad ref -> 6A88", s, "80EC010A08" + "3131313131313131", 0x6A88);
        swOn("hardening: SET PIN for ADM1 -> 6A88", s, "80EC000A08" + "3131313131313131", 0x6A88);
        swOn("hardening: SET PIN bad P1 -> 6B00", s, "80EC020108" + "3131313131313131", 0x6B00);
        swOn("hardening: SET PUK1", s, "80EC010108" + "3131313131313131", 0x9000);
        swOn("hardening: UNBLOCK PIN1 with old PUK -> 63C9", s, "002C000110" + "3132333435363738" + "31323334FFFFFFFF", 0x63C9);
        swOn("hardening: UNBLOCK PIN1 with new PUK", s, "002C000110" + "3131313131313131" + "31323334FFFFFFFF", 0x9000);
        swOn("hardening: SET PUK2", s, "80EC018108" + "3232323232323232", 0x9000);
        swOn("hardening: PUK2 retries reset -> 63CA", s, "002C008100", 0x63CA);
        swOn("hardening: wrong PIN2 -> 63C2", s, "0020008108" + "31313131FFFFFFFF", 0x63C2);
        swOn("hardening: SET PIN2 = 9999", s, "80EC008108" + "39393939FFFFFFFF", 0x9000);
        swOn("hardening: PIN2 retries reset -> 63C3", s, "0020008100", 0x63C3);
        swOn("hardening: old PIN2 refused -> 63C2", s, "0020008108" + "32323232FFFFFFFF", 0x63C2);
        swOn("hardening: new PIN2", s, "0020008108" + "39393939FFFFFFFF", 0x9000);

        swOn("hardening: preperso EF_SPC = 123456", s, "80F46F3303" + "123456", 0x9000);
        for (int i = 0; i < 10; i++) {
            data("hardening: wrong SPC " + (i + 1) + " -> 0B", txOn(s, "80CE000005" + "0003999999"), "000B");
        }
        data("hardening: right SPC after 10 failures -> 0A", txOn(s, "80CE000005" + "0003123456"), "000A");
        swOn("hardening: ADM1 rewrite of EF_SPC", s, "80F46F3303" + "123456", 0x9000);
        data("hardening: right SPC after rewrite -> accept", txOn(s, "80CE000005" + "0003123456"), "0000");

        if (!org.onexbts.csim.Variant.DEBUG) {
            swOn("hardening: PLATFORM_INFO with ADM1", s, "80FE000000", 0x9000);
        }
        s.reset();
        txOn(s, concat("00A40404", AID_BYTES, "00"));
        swOn("hardening: ADM1 cleared by reset (SET PUK -> 6982)", s, "80EC010108" + "3131313131313131", 0x6982);
        txOn(s, "00A4000C026F22");
        swOn("hardening: PIN1 cleared by reset (read IMSI_M -> 6982)", s, "00B000000A", 0x6982);
        if (!org.onexbts.csim.Variant.DEBUG) {
            swOn("hardening: PLATFORM_INFO without ADM1 -> 6982", s, "80FE000000", 0x6982);
        }

        // Handset writes without ADM1 take the journaled path.
        swOn("tear-safe: verify PIN1", s, "0020000108313233 34FFFFFFFF".replace(" ", ""), 0x9000);
        swOn("tear-safe: select EF_COUNT", s, "00A4000C026F21", 0x9000);
        data("tear-safe: INCREASE COUNT by 5", txOn(s, "003200000300000500"), "0005 000005");
        swOn("tear-safe: UPDATE RECORD previous (cyclic)", s, "00DC0003020007", 0x9000);
        data("tear-safe: COUNT record 1 is 0007", txOn(s, "00B2010402"), "0007");
        data("tear-safe: COUNT record 2 is 0005", txOn(s, "00B2020402"), "0005");
        swOn("tear-safe: select EF_ICI", s, "00A4000C026F7C", 0x9000);
        byte[] recC = new byte[44]; java.util.Arrays.fill(recC, (byte) 0xCC);
        swOn("tear-safe: ICI write (previous)", s, concat("00DC0003", recC, ""), 0x9000);
        data("tear-safe: ICI record 1", txOn(s, "00B201042C"), hex(recC));
        swOn("tear-safe: select EF_TMSI", s, "00A4000C026F24", 0x9000);
        swOn("tear-safe: UPDATE BINARY TMSI", s, "00D6000004DEADBEEF", 0x9000);
        data("tear-safe: TMSI read back", txOn(s, "00B0000004"), "DEADBEEF");
        data("tear-safe: store ESN -> changed", txOn(s, "80DE000008040234 34F0000000".replace(" ", "")), "01");
        data("tear-safe: store same ESN -> unchanged", txOn(s, "80DE000008040234 34F0000000".replace(" ", "")), "00");
        swOn("tear-safe: select ESN_ME", s, "00A4000C026F38", 0x9000);
        data("tear-safe: ESN_ME content", txOn(s, "00B0000008"), "0402343 4F0000000".replace(" ", ""));
    }

    static ResponseAPDU txOn(CardSimulator s, String apdu) {
        return s.transmitCommand(new CommandAPDU(hex(apdu)));
    }
    static ResponseAPDU txOn(CardSimulator s, byte[] apdu) {
        return s.transmitCommand(new CommandAPDU(apdu));
    }
    static void swOn(String name, CardSimulator s, String apdu, int expect) {
        ResponseAPDU r = txOn(s, apdu);
        check(name, r.getSW() == expect, String.format("SW=%04X expected %04X data=%s", r.getSW(), expect, hex(r.getData())));
    }
    static void swOn(String name, CardSimulator s, byte[] apdu, int expect) {
        ResponseAPDU r = txOn(s, apdu);
        check(name, r.getSW() == expect, String.format("SW=%04X expected %04X data=%s", r.getSW(), expect, hex(r.getData())));
    }
    static byte[] cat(byte[]... parts) {
        int n = 0;
        for (byte[] p : parts) n += p.length;
        byte[] out = new byte[n];
        int o = 0;
        for (byte[] p : parts) { System.arraycopy(p, 0, out, o, p.length); o += p.length; }
        return out;
    }

    static byte[] slice(byte[] b, int off, int n) {
        byte[] out = new byte[n];
        System.arraycopy(b, off, out, 0, n);
        return out;
    }
    static byte[] concat(String headHex, byte[] body, String tailHex) {
        byte[] head = hex(headHex);
        byte[] tail = hex(tailHex);
        byte[] out = new byte[head.length + 1 + body.length + tail.length];
        System.arraycopy(head, 0, out, 0, head.length);
        out[head.length] = (byte) body.length;
        System.arraycopy(body, 0, out, head.length + 1, body.length);
        System.arraycopy(tail, 0, out, head.length + 1 + body.length, tail.length);
        return out;
    }
    static byte[] concat(byte[] head, byte[] body, String tailHex) {
        byte[] tail = hex(tailHex);
        byte[] out = new byte[head.length + body.length + tail.length];
        System.arraycopy(head, 0, out, 0, head.length);
        System.arraycopy(body, 0, out, head.length, body.length);
        System.arraycopy(tail, 0, out, head.length + body.length, tail.length);
        return out;
    }
}
