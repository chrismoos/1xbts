# CSIM applet for sysmoISIM-SJA5

A CSIM application (C.S0065-B) for the sysmoISIM-SJA5 card, and `csim`, a
tool that installs and personalizes it.

## Requirements

- A sysmoISIM-SJA5 and its key material, which sysmocom sends by email
- A PC/SC card reader. On Linux: `libpcsclite-dev` and a running `pcscd`
- Rust, Java, GlobalPlatformPro (`gp.jar`) and Git LFS

## Bring up a card

1. Install `csim`:

   ```sh
   cargo install --path crates/csim --features csim
   ```

2. Download GlobalPlatformPro:

   ```sh
   curl -L -o gp.jar \
       https://github.com/martinpaljak/GlobalPlatformPro/releases/download/v25.10.20/gp.jar
   ```

3. Set the card's keys from the key material:

   ```sh
   export KIC1=<KIC1> KID1=<KID1> KIK1=<KIK1>
   export PLATFORM_ADM1=<ADM1>
   ```

4. Install the applet:

   ```sh
   csim install
   ```

5. Fill the profile template for the card's IMSI and write the profile:

   ```sh
   csim profile sim/examples/card.tmpl.yaml --imsi 001010000000042 --mdn 5550142
   csim write sim/examples/card-001010000000042.local.yaml
   ```

> A card allows about ten failed GlobalPlatform logins before it locks.
> Stop at the first `6982` error and check the keys.

`csim install` erases the profile, so run `csim write` again
after every install. With more than one reader attached, set `CSIM_READER` to part of
the card reader's name.

## Profiles

Edit `sim/examples/card.tmpl.yaml`, or copy `sim/examples/card.yaml` for a
profile without placeholders. Every key and placeholder is listed in
[docs/profiles.md](docs/profiles.md). A filled profile holds the card's
A-key and IP secrets, which the network must also be given.

## More

- [docs/csim.md](docs/csim.md): commands, options and install errors
- [docs/profiles.md](docs/profiles.md): profile keys

## CSIM Applet

The applet implements the C.S0065-B file system, the TS 102 221 command
set, PIN1, PIN2 and ADM1, CAVE authentication, OTASP/OTAPA and packet-data
IP authentication.

### Not implemented

- AKA, Secure Mode and the OTASP A-key Diffie-Hellman exchange, which the
  SJA5 cannot run
- BCMCS, Application Authentication, LCS and TLS commands, which answer
  `6D00`

### Build

The bundled CAP, `sim/applet/dist/csim.cap`, is enough to bring up a card.
Building is needed only to change the applet.

Requirements:

- Java Card 3.0.4 SDK (`jc304_kit`). The SJA5 does not load CAPs built with
  newer SDKs
- JDK 8, and JDK 17 for Gradle

```sh
git clone --depth 1 https://github.com/martinpaljak/oracle_javacard_sdks \
    ~/Storage/opensource/oracle_javacard_sdks
export JC_HOME=~/Storage/opensource/oracle_javacard_sdks/jc304_kit
export JAVA8_HOME=/path/to/jdk8   # default: /Library/Java/JavaVirtualMachines/zulu-8.jdk/Contents/Home

(cd sim/applet && ./gradlew dist)
csim install --cap sim/applet/build/csim.cap
```

Copy `sim/applet/build/csim.cap` to `sim/applet/dist/csim.cap` to replace
the bundled CAP.

`./gradlew dist -Pdebug=true` builds the debug applet. It adds the
`apdulog` trace, `verify-meta` and the S.S0055 self-test, and answers
`platform-info` without ADM1. The default build, and the bundled CAP, answer
`6D00` to those commands.

### Tests

```sh
cargo test --release -p csim
(cd sim/applet && ./gradlew simtest -Pss0055=int)
(cd sim/applet && ./gradlew simtest -Pss0055=int -Pdebug=true)
```

`simtest` runs the applet in a simulator and prints one PASS or FAIL line
per check. The simulator has no memory limit, so a build that passes can
still fail to install on a card.
