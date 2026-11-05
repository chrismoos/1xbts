# csim

## Commands

| Command | Action |
|---|---|
| `install` | Install the applet, register it in EF_DIR, load the EF table, set the applet's ADM1 to the platform ADM1 and disable PIN1 |
| `write <profile>` | Write a profile and disable PIN1 |
| `verify <profile>` | Write, read back and compare. `--save-dump` also checks every written byte |
| `validate <profile>` | Check a profile without a card |
| `profile <template> --imsi <IMSI>` | Fill a profile template for one subscriber. See [profiles.md](profiles.md#templates) |
| `read` | Read and decode every EF |
| `dump --save <file>` | Save every file, access condition and PIN status of any CSIM. `--scan` also tries every FID in `6F00..6FFF` and `2F00..2FFF` |
| `compare <a> <b>` | Compare two dumps. `--content` also compares content |
| `compare <dump> --profile <profile>` | Check a dump against a profile |
| `compare <dump> --table` | Check a dump against the EF table |
| `info` | Show the ATR (Answer To Reset), whether a CSIM is present and the files readable without a PIN |
| `register-dir --platform-adm1 <ADM1>` | Add the CSIM to the card's EF_DIR. `--first` puts it in record 1 |
| `init-meta` | Load the EF table |
| `chpin pin1\|pin2\|puk1\|puk2 --new <value>` | Set a PIN (4-8 digits) or PUK (8 digits) with ADM1. Also resets its retry counter |
| `chpin adm1 --new <value>` | Change ADM1. `--current` gives the current ADM1 when it differs from `--adm1` |
| `chpin pin1 --disable`, `--enable` | Disable or enable PIN1. Needs the current PIN1 |
| `unblock pin1\|pin2 --puk <puk>` | Reset a PIN's retry counter. `--new-pin` also sets a new value |
| `akey-checksum --esn <hex>` | Print the 6-digit checksum of the stored A-key |
| `apdulog` | Print the last 32 commands the applet received. `--clear` empties it. Debug build only |
| `platform-info` | Print the Java Card version and free memory |
| `verify-meta` | Read back the EF table. Debug build only |

## Options

Card commands take `--reader`, `--adm1`, `--pin1` and `--trace`. PIN1 and
PIN2 default to the applet's factory values, `1234` and `2222`.
`--reader sim` uses a simulated card, which starts empty for each command.

| Option | Environment | Default |
|---|---|---|
| `--reader` | `CSIM_READER` | the only card reader attached |
| `--adm1` | `PLATFORM_ADM1` | none, required by commands that log in with ADM1 |
| `--platform-adm1` | `PLATFORM_ADM1` | none, required by `install` and `register-dir` |
| `--kic1`, `--kid1`, `--kik1` | `KIC1`, `KID1`, `KIK1` | none, required by `install` |
| `--key-ver` | `GP_KEY_VER` | `0x70` |
| `--gp-jar` | `GP_JAR` | `gp.jar` in the current directory |
| `--gp-reader` | `GP_READER` | the reader `csim` uses |
| `--cap` | | `sim/applet/dist/csim.cap` |
| `--keep-pin1` | | off: `install` and `write` disable PIN1 |

## ADM1

`install` sets the applet's ADM1 to the platform ADM1, 8 digits. ADM1 has
10 tries and no unblock code. `dump` takes another vendor's ADM1 with
`--adm1-hex`.

## Install

`install` replaces any installed copy of the applet and erases its profile
and PINs. The EF_DIR entry is kept.

PIN1 is disabled by default because handsets do not prompt for the CSIM's
PIN1, so a card with PIN1 enabled gets no service.

## Check a card

```sh
csim dump --pin1 1234 --adm1 "$PLATFORM_ADM1" --save sim/dumps/ours.yaml
csim compare sim/dumps/ours.yaml --profile sim/examples/card.yaml
csim compare sim/dumps/ours.yaml --table
```

Dumps contain the card's identities and must not be committed.

## Install errors

| Error | Cause |
|---|---|
| `6982`, "Card cryptogram invalid" | Wrong GlobalPlatform keys. Each attempt uses one of about ten |
| `6A88` | No keyset at the requested version. The SJA5 uses `0x70` |
| `6985` on LOAD | The CAP was built with a Java Card SDK newer than 3.0.4 |
| `6A80` on INSTALL | The applet allocates too much memory at install |
| `SCARD_E_NOT_TRANSACTED` | macOS reader session problem. Remove and reinsert the card, or run `sudo killall -KILL com.apple.ifdreader` |
