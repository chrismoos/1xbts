import com.licel.jcardsim.smartcardio.CardSimulator;
import com.licel.jcardsim.utils.AIDUtil;
import javacard.framework.AID;
import javax.smartcardio.CommandAPDU;
import javax.smartcardio.ResponseAPDU;
import java.io.BufferedReader;
import java.io.InputStreamReader;

/**
 * Runs the CSIM applet in jcardsim and answers APDUs read from stdin,
 * one hex string per line, with the response data and status word as
 * one hex string per line. The line "RESET" reinstalls the applet.
 * The `csim` CLI uses this as its `--reader sim` transport.
 */
public class ApduServer {
    static final byte[] AID_BYTES = SimTest.hex("A0000003431002F310FFFF89020000FF");

    static CardSimulator fresh() {
        CardSimulator sim = new CardSimulator();
        sim.installApplet(AIDUtil.create(AID_BYTES), org.onexbts.csim.Csim.class);
        return sim;
    }

    public static void main(String[] args) throws Exception {
        CardSimulator sim = fresh();
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in));
        System.out.println("READY");
        System.out.flush();
        String line;
        while ((line = in.readLine()) != null) {
            line = line.trim();
            if (line.isEmpty()) continue;
            if (line.equals("RESET")) {
                sim = fresh();
                System.out.println("9000");
                System.out.flush();
                continue;
            }
            ResponseAPDU r;
            try {
                r = sim.transmitCommand(new CommandAPDU(SimTest.hex(line)));
            } catch (Exception e) {
                System.out.println("ERR " + e);
                System.out.flush();
                continue;
            }
            System.out.println(SimTest.hex(r.getData()) + String.format("%04X", r.getSW()));
            System.out.flush();
        }
    }
}
