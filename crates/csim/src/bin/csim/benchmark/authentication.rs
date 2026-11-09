use super::*;
use cdma_cave::{
    AKey, Esn, RandSsd, Ssd, auth_signature, authbs, generate_call_keys, generate_ssd,
};
use csim::profile::Profile;
use md5::{Digest, Md5};

const INS_CHALLENGE: u8 = 0x8A;
const INS_SSD: u8 = 0x82;
const INS_IP: u8 = 0x80;
const DEFER_COMMIT: u8 = 4;
const CONFIRM: u8 = 1;
const AUTH_COMPARE_FAILED: u16 = 0x9804;
const IMSI_M: Fid = Fid(0x6F22);
const IMSI_LEN: usize = 10;
const PROBE_RAND: u32 = 0x67EC62A5;
const CHALLENGE: &[u8] = &[0x25, 0x2D, 0x57, 0x00, 0xF6, 0xFF, 0x23];
const CHAP_ID: u8 = 5;
const HASH_LEN: usize = 16;
const CHAP: u8 = 0;
const MN_HA: u8 = 1;
const RRQ: u8 = 2;
const MN_AAA: u8 = 3;
const HRPD: u8 = 4;
const FIRST: u8 = 0;
const NEXT: u8 = 1;
const SINGLE: u8 = 2;
const LAST: u8 = 3;

fn apdu(ins: u8, p1: u8, p2: u8, data: &[u8], response: bool) -> Vec<u8> {
    let mut out = vec![
        CLA_CSIM,
        ins,
        p1,
        p2,
        u8::try_from(data.len()).expect("short APDU"),
    ];
    out.extend(data);
    if response {
        out.push(0);
    }
    out
}

fn equal(actual: &[u8], expected: &[u8], label: &str) -> Result<()> {
    if actual != expected {
        return Err(format!("{label}: host reference mismatch (values omitted)").into());
    }
    Ok(())
}

fn signature(bench: &mut Bench, esn: [u8; 4]) -> Result<Vec<u8>> {
    bench.command(
        "ssd-auth-probe",
        &auth_apdu(esn, PROBE_RAND, RAND_GLOBAL, SAVE_REGISTERS, &[0; 4]),
        SIGNATURE_LEN,
    )
}

fn verify_active(
    bench: &mut Bench,
    esn: [u8; 4],
    min1: u32,
    min2_low: u8,
    ssd: &Ssd,
) -> Result<()> {
    let esn_value = Esn(u32::from_be_bytes(esn));
    for (name, kind, digits, rand, auth_data) in [
        ("registration", RAND_GLOBAL, [0; 4], PROBE_RAND, min1),
        (
            "origination",
            RAND_GLOBAL,
            [20, 2, 0x22, 0x22],
            PROBE_RAND,
            0x022222,
        ),
        (
            "unique",
            RAND_UNIQUE,
            [0; 4],
            (PROBE_RAND << 8) | u32::from(min2_low),
            min1,
        ),
    ] {
        let expected = auth_signature(rand, auth_data, &ssd.ssd_a, esn_value)?;
        let actual = bench.command(
            &format!("ssd-reference-{name}"),
            &auth_apdu(esn, PROBE_RAND, kind, SAVE_REGISTERS, &digits),
            SIGNATURE_LEN,
        )?;
        equal(&actual, &expected.signature.to_be_bytes()[1..], name)?;
        let keys = generate_call_keys(&expected.saved, &ssd.ssd_b, esn_value);
        let mut bytes = keys.cmea_key.0.to_vec();
        bytes.extend(keys.vpm);
        equal(
            &bench.generate(&format!("ssd-reference-{name}-full-vpm"), 0, VPM_LAST)?,
            &bytes,
            "active SSD key/VPM",
        )?;
    }
    Ok(())
}

fn ssd(bench: &mut Bench, args: &BenchArgs, esn: [u8; 4], profile: Option<&Profile>) -> Result<()> {
    let key = profile
        .and_then(|p| p.a_key.as_ref())
        .map(|v| {
            csim::akey::parse_a_key(&v.text())
                .map(AKey::from_u64)
                .map_err(|_| "invalid profile A-key")
        })
        .transpose()?;
    bench.measure("ssd-select-imsi", |c| Ok(c.select_fid(IMSI_M)?))?;
    let imsi = bench.measure("ssd-read-imsi", |c| Ok(c.read_binary(IMSI_LEN, 0)?))?;
    if imsi.len() != IMSI_LEN {
        return Err("short EF_IMSI_M".into());
    }
    let min1 = u32::from_le_bytes([imsi[3], imsi[4], imsi[5], 0]);
    if args.commit_ssd && key.is_none() {
        return Err("--commit-ssd requires profile A-key".into());
    }
    for iteration in 0..args.iterations {
        let before = signature(bench, esn)?;
        let seed = PROBE_RAND.wrapping_add(u32::from(iteration)).to_be_bytes();
        let bs = bench.command(
            "ssd-base-station-challenge",
            &apdu(INS_CHALLENGE, 0, 0, &seed, true),
            4,
        )?;
        let Some(key) = key.as_ref() else {
            println!("coverage,ssd-update-confirm,SKIP,profile A-key required");
            break;
        };
        let randssd = RandSsd::from_u64(0x252D5700F6FF23 + u64::from(iteration))?;
        let candidate = generate_ssd(key, Esn(u32::from_be_bytes(esn)), &randssd);
        let mut data = randssd.0.to_vec();
        data.push(if args.commit_ssd { 0 } else { DEFER_COMMIT });
        data.extend(esn.into_iter().rev());
        data.extend([0; 3]);
        bench.command("ssd-update", &apdu(INS_SSD, 0, 0, &data, false), 0)?;
        let randbs = u32::from_be_bytes(bs.try_into().map_err(|_| "short RANDBS")?);
        let expected = authbs(&candidate.ssd_a, randbs, min1, Esn(u32::from_be_bytes(esn)))?;
        let wrong = (expected ^ 1).to_be_bytes();
        bench.measure("ssd-reject-wrong-authbs", |c| {
            let response = c.transmit(&apdu(INS_SSD, 0, CONFIRM, &wrong[1..], false))?;
            if response.sw != AUTH_COMPARE_FAILED || !response.data.is_empty() {
                return Err(format!("wrong AUTHBS: expected 9804, got {:04X}", response.sw).into());
            }
            Ok(())
        })?;
        equal(
            &signature(bench, esn)?,
            &before,
            "rejected confirmation preserved SSD",
        )?;
        bench.command(
            "ssd-confirm",
            &apdu(INS_SSD, 0, CONFIRM, &expected.to_be_bytes()[1..], false),
            0,
        )?;
        if args.commit_ssd {
            println!("state,SSD committed,network resynchronization required");
            verify_active(bench, esn, min1, imsi[1], &candidate)?;
        } else {
            equal(
                &signature(bench, esn)?,
                &before,
                "deferred confirmation preserved SSD",
            )?;
        }
    }
    if key.is_some() {
        println!(
            "coverage,ssd-update-confirm,PASS,{}",
            if args.commit_ssd {
                "committed AUTHR and full VPM verified"
            } else {
                "deferred candidate verified active SSD unchanged"
            }
        );
    }
    Ok(())
}

fn digest(parts: &[&[u8]]) -> Vec<u8> {
    let mut hash = Md5::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().to_vec()
}

fn ip_command(
    bench: &mut Bench,
    label: &str,
    method: u8,
    block: u8,
    data: &[u8],
    length: usize,
) -> Result<Vec<u8>> {
    bench.command(
        &format!("{label}-{}B", data.len()),
        &apdu(INS_IP, method, block, data, length > 0),
        length,
    )
}

fn rrq(bench: &mut Bench, data: &[u8], chained: bool) -> Result<()> {
    if chained {
        ip_command(bench, "ip-rrq-first", RRQ, FIRST, &data[..1], 0)?;
        ip_command(bench, "ip-rrq-next", RRQ, NEXT, &data[1..3], 0)?;
        ip_command(bench, "ip-rrq-last", RRQ, LAST, &data[3..], 0)?;
    } else {
        ip_command(bench, "ip-rrq-single", RRQ, SINGLE, data, 0)?;
    }
    Ok(())
}

fn ip_secret(bench: &mut Bench, secret: &csim::profile::IpSecret, iterations: u16) -> Result<()> {
    let class = secret.class_number().ok_or("invalid IP secret class")?;
    let key = secret.bytes().map_err(|_| "invalid IP secret encoding")?;
    for length in [4, 16, 64, 200] {
        let challenge: Vec<u8> = (0..length)
            .map(|v| (v as u8).wrapping_add(CHAP_ID))
            .collect();
        let challenge = challenge.as_slice();
        let mut indexed = vec![secret.index];
        indexed.extend(challenge);
        for _ in 0..iterations {
            match class {
                CHAP | 3 => {
                    if class == 3 && secret.index != 0 {
                        return Err("HRPD only supports index zero".into());
                    }
                    let mut input = vec![CHAP_ID];
                    if class == CHAP {
                        input.push(secret.index);
                    }
                    input.extend(challenge);
                    let result = ip_command(
                        bench,
                        if class == CHAP { "ip-chap" } else { "ip-hrpd" },
                        if class == CHAP { CHAP } else { HRPD },
                        0,
                        &input,
                        HASH_LEN,
                    )?;
                    equal(
                        &result,
                        &digest(&[&[CHAP_ID], &key, challenge]),
                        "CHAP/HRPD",
                    )?;
                }
                MN_HA => {
                    let expected = digest(&[&key, challenge, &key]);
                    equal(
                        &ip_command(bench, "ip-mn-ha-single", MN_HA, SINGLE, &indexed, HASH_LEN)?,
                        &expected,
                        "MN-HA single",
                    )?;
                    ip_command(bench, "ip-mn-ha-first", MN_HA, FIRST, &indexed[..2], 0)?;
                    ip_command(bench, "ip-mn-ha-next", MN_HA, NEXT, &indexed[2..4], 0)?;
                    equal(
                        &ip_command(bench, "ip-mn-ha-last", MN_HA, LAST, &indexed[4..], HASH_LEN)?,
                        &expected,
                        "MN-HA chained",
                    )?;
                }
                2 => {
                    let expected =
                        digest(&[&challenge[..1], &key, &digest(&[challenge]), challenge]);
                    for chained in [false, true] {
                        rrq(bench, challenge, chained)?;
                        equal(
                            &ip_command(bench, "ip-mn-aaa", MN_AAA, 0, &indexed, HASH_LEN)?,
                            &expected,
                            "MN-AAA",
                        )?;
                    }
                }
                _ => return Err("unsupported IP secret class".into()),
            }
        }
    }
    Ok(())
}

pub(super) fn exercise(
    bench: &mut Bench,
    args: &BenchArgs,
    esn: [u8; 4],
    profile: Option<&Profile>,
) -> Result<()> {
    let mut failures = 0;
    if let Err(error) = ssd(bench, args, esn, profile) {
        eprintln!("coverage,ssd,FAIL,{error}");
        failures += 1;
    }
    if let Err(error) = rrq(bench, CHALLENGE, false).and_then(|()| rrq(bench, CHALLENGE, true)) {
        eprintln!("coverage,ip-rrq,FAIL,{error}");
        failures += 1;
    } else {
        println!("coverage,ip-rrq,PASS,status only unless MN-AAA credentials supplied");
    }
    for class in 0..4 {
        let secrets: Vec<_> = profile
            .and_then(|p| p.ip_secrets.as_ref())
            .into_iter()
            .flatten()
            .filter(|s| s.class_number() == Some(class))
            .collect();
        if secrets.is_empty() {
            println!("coverage,ip-class-{class},SKIP,no profile credential");
        }
        for secret in secrets {
            match ip_secret(bench, secret, args.iterations) {
                Ok(()) => println!(
                    "coverage,ip-class-{class}-index-{},PASS,host reference matched",
                    secret.index
                ),
                Err(error) => {
                    eprintln!(
                        "coverage,ip-class-{class}-index-{},FAIL,{error}",
                        secret.index
                    );
                    failures += 1;
                }
            }
        }
    }
    println!("coverage,AKA-OTASP-provisioning-PIN-mutations,SKIP,not exercised by this suite");
    if !args.commit_ssd {
        println!(
            "coverage,active-ssd-commit,SKIP,requires --commit-ssd and changes authentication state"
        );
    }
    if failures > 0 {
        return Err(format!("{failures} authentication suite(s) failed").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires Java and the applet Gradle simulator"]
    fn simulator_checks_ssd_commit_defer_and_all_ip_classes() -> Result<()> {
        let mut card = Card::new(Box::new(csim::sim::SimTransport::start()?), false);
        card.select_csim()?;
        card.verify(csim::card::KEY_REF_ADM1, &[0; 8], true)?;
        card.ensure_ef_table(&csim::ef_table::meta_bytes())?;
        card.verify(KEY_REF_PIN1, &pin_bytes("1234")?, true)?;
        card.select_fid(IMSI_M)?;
        card.update_binary(&[0, 0xE7, 3, 0x78, 0x17, 0xEC, 0, 0, 0, 0], 0)?;
        card.load_a_key(0xC442F56BE9E17158)?;
        let profile: Profile = serde_yaml::from_str(
            "a_key: 'C442F56BE9E17158'\nip_secrets:\n  - {class: 0, index: 0, secret_text: benchmark}\n  - {class: 1, index: 0, secret_text: benchmark}\n  - {class: 2, index: 0, secret_text: benchmark}\n  - {class: 3, index: 0, secret_text: benchmark}\n",
        )?;
        for secret in profile.ip_secrets.as_ref().unwrap() {
            card.load_ip_secret(
                secret.class_number().unwrap(),
                secret.index,
                &secret.bytes().unwrap(),
            )?;
        }
        let mut bench = Bench {
            card,
            measurements: Rc::new(RefCell::new(Measurements::default())),
        };
        let mut args = BenchArgs {
            reader: None,
            pin1: Some("1234".into()),
            esn: "8069398A".into(),
            iterations: 2,
            profile: None,
            commit_ssd: false,
            check_count: false,
        };
        let esn = [0x80, 0x69, 0x39, 0x8A];
        exercise(&mut bench, &args, esn, Some(&profile))?;
        args.commit_ssd = true;
        exercise(&mut bench, &args, esn, Some(&profile))?;
        let wrong: Profile = serde_yaml::from_str("a_key: '0000000000000001'")?;
        assert!(ssd(&mut bench, &args, esn, Some(&wrong)).is_err());
        Ok(())
    }
}
