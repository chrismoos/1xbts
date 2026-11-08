use super::*;

const EF_COUNT: Fid = Fid(0x6F21);
const RECORD_LENGTH: u8 = 2;
const RECORD_COUNT: u8 = 10;
const INS_INCREASE: u8 = 0x32;
const RECORD_PREVIOUS: u8 = 3;
const INCREMENT: [u8; 3] = [0, 0, 1];

fn select(bench: &mut Bench, pin: &[u8]) -> Result<()> {
    bench.measure("count-select-csim", |c| Ok(c.select_csim()?))?;
    bench.measure("count-verify-pin1", |c| {
        Ok(c.verify(KEY_REF_PIN1, pin, true)?)
    })?;
    bench.measure("count-select-ef", |c| Ok(c.select_fid(EF_COUNT)?))
}

fn records(bench: &mut Bench) -> Result<Vec<Vec<u8>>> {
    (1..=RECORD_COUNT)
        .map(|record| {
            bench.measure("count-read-record", |c| {
                Ok(c.read_record(record, RECORD_LENGTH)?)
            })
        })
        .collect()
}

fn write(bench: &mut Bench, value: &[u8]) -> Result<()> {
    let mut apdu = vec![
        CLA_UICC,
        csim::card::INS_UPDATE_RECORD,
        0,
        RECORD_PREVIOUS,
        RECORD_LENGTH,
    ];
    apdu.extend_from_slice(value);
    bench.command("count-update-previous", &apdu, 0)?;
    Ok(())
}

fn check(bench: &mut Bench, expected: &[Vec<u8>]) -> Result<()> {
    let actual = records(bench)?;
    if actual != expected {
        return Err(
            format!("COUNT records differ: expected={expected:02X?} actual={actual:02X?}").into(),
        );
    }
    Ok(())
}

fn prepend(expected: &mut Vec<Vec<u8>>, value: Vec<u8>) {
    expected.insert(0, value);
    expected.pop();
}

pub(super) fn exercise(bench: &mut Bench, args: &BenchArgs) -> Result<()> {
    if !args.check_count {
        println!("check,count,SKIPPED,use --check-count for physical COUNT write checks");
        return Ok(());
    }
    let pin = pin_bytes(args.pin1.as_deref().unwrap_or(crate::DEFAULT_PIN1))?;
    println!(
        "check,count-reset,SKIPPED,reinsert card before testing to clear prior verification state"
    );
    select(bench, &pin)?;
    let original = records(bench)?;
    if original
        .iter()
        .any(|record| record.len() != usize::from(RECORD_LENGTH))
    {
        return Err("COUNT record length differs from the expected two bytes".into());
    }
    println!("count-backup,{original:02X?}");
    let result = (|| -> Result<()> {
        let mut expected = original.clone();
        write(bench, &[0, 0])?;
        prepend(&mut expected, vec![0, 0]);
        check(bench, &expected)?;
        for value in 1..=RECORD_COUNT + 2 {
            let mut apdu = vec![CLA_UICC, INS_INCREASE, 0, 0, INCREMENT.len() as u8];
            apdu.extend(INCREMENT);
            apdu.push(0);
            let response = bench.command(
                "count-increase",
                &apdu,
                RECORD_LENGTH as usize + INCREMENT.len(),
            )?;
            let mut wanted = vec![0, value];
            wanted.extend(INCREMENT);
            if response != wanted {
                return Err(format!(
                    "COUNT INCREASE expected={wanted:02X?} actual={response:02X?}"
                )
                .into());
            }
            prepend(&mut expected, vec![0, value]);
            check(bench, &expected)?;
            select(bench, &pin)?;
            check(bench, &expected)?;
        }
        println!("check,count-writes-reselect-ring-wrap,PASS");
        const BOUNDARY_STARTS: [u16; 3] = [62, 254, u16::MAX - 1];
        const COUNT_MASK: u16 = 63;
        const SW_INCREASE_OVERFLOW: u16 = 0x9850;
        for start in BOUNDARY_STARTS {
            write(bench, &start.to_be_bytes())?;
            prepend(&mut expected, start.to_be_bytes().to_vec());
            check(bench, &expected)?;
            let mut value = start;
            for _ in 0..2 {
                let mut apdu = vec![CLA_UICC, INS_INCREASE, 0, 0, INCREMENT.len() as u8];
                apdu.extend(INCREMENT);
                apdu.push(0);
                if let Some(next) = value.checked_add(1) {
                    let response = bench.command(
                        "count-boundary-increase",
                        &apdu,
                        RECORD_LENGTH as usize + INCREMENT.len(),
                    )?;
                    let mut wanted = next.to_be_bytes().to_vec();
                    wanted.extend(INCREMENT);
                    if response != wanted {
                        return Err(format!("COUNT boundary {value:04X}: expected={wanted:02X?} actual={response:02X?}").into());
                    }
                    println!(
                        "check,count-boundary,{value:04X}->{next:04X},low6={}->{}",
                        value & COUNT_MASK,
                        next & COUNT_MASK
                    );
                    value = next;
                    prepend(&mut expected, next.to_be_bytes().to_vec());
                } else {
                    bench.measure("count-boundary-overflow", |card| {
                        let response = card.transmit(&apdu)?;
                        if response.sw != SW_INCREASE_OVERFLOW {
                            return Err(format!(
                                "COUNT overflow expected 9850, got {:04X}",
                                response.sw
                            )
                            .into());
                        }
                        Ok(())
                    })?;
                    println!("check,count-overflow-rejected,PASS");
                }
                check(bench, &expected)?;
                select(bench, &pin)?;
                check(bench, &expected)?;
            }
        }
        for value in [COUNT_MASK, 0] {
            write(bench, &value.to_be_bytes())?;
            prepend(&mut expected, value.to_be_bytes().to_vec());
            check(bench, &expected)?;
            select(bench, &pin)?;
            check(bench, &expected)?;
        }
        println!("check,count-explicit-mod64-update,PASS");
        println!("check,count-boundaries,PASS");
        Ok(())
    })();
    let restored = (|| -> Result<()> {
        select(bench, &pin)?;
        for record in original.iter().rev() {
            write(bench, record)?;
        }
        check(bench, &original)?;
        println!("check,count-original-records-restored,PASS");
        Ok(())
    })();
    if let Err(error) = restored {
        return Err(format!("COUNT restoration failed: {error}. Original records are in count-backup. Test result: {result:?}").into());
    }
    result
}
