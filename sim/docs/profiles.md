# Profiles

A profile is a YAML file. `sim/examples/card.yaml` shows every key with a
comment. `prl_file` and `eprl_file` paths are relative to the profile.
Section numbers refer to C.S0065-B.

| Key | EF | Value |
|---|---|---|
| `pin1`, `pin2` | none | 4 to 8 digits. Sets PIN1 and PIN2. Unset keeps the card's values |
| `puk1`, `puk2` | none | 8 digits. Sets PUK1 and PUK2. Default: `12345678` and `87654321` |
| `iccid` | EF_ICCID | 19 or 20 digits including the Luhn check digit, §5.2.91 |
| `imsi_m`, `imsi_t` | EF_IMSI_M, EF_IMSI_T | 15-digit IMSI, §5.2.2 |
| `mdn` | EF_MDN | up to 15 of `0-9 * #`, §5.2.35 |
| `cdmahome` | EF_CDMAHOME | list of `{sid, nid, band}`, up to 5, §5.2.8 |
| `accolc` | EF_ACCOLC | 0-15, §5.2.12 |
| `spc` | EF_SPC | six-digit service programming code, quoted, §5.2.19. Default `000000`. After 10 wrong OTASP attempts the SPC is refused until `csim write` writes it again |
| `prl_file`, `eprl_file` | EF_PRL, EF_EPRL | binary PRL file. `prl` and `eprl` take hex. An extended PRL goes in `eprl_file` |
| `ecc` | EF_ECC | up to 5 emergency numbers of up to 6 digits, §5.2.38 |
| `ah` | EF_AH | analog home SID, §5.2.5 |
| `language` | EF_LI | ISO 639-1 code or list, §5.2.26 |
| `operation_mode` | EF_AD | default 0, §5.2.34 |
| `services` | EF_CSIM_ST | service numbers, §5.2.18 |
| `enabled_services` | EF_EST | enabled service numbers, §5.2.80 |
| `spn` | EF_SPN | `name`, `display_condition`, `char_encoding`, `language_indicator`, §5.2.32 |
| `usgind` | EF_USGIND | `use_uim_id`, `use_sf_euimid`, §5.2.33 |
| `esn`, `meid` | EF_ESN_ME | 8 or 14 hex digits, §5.2.24. The handset rewrites it at power-up |
| `ruimid` | EF_RUIMID | 32-bit UIM_ID. Default: derived from `sf_euimid` or `iccid`, §5.2.17 |
| `sf_euimid` | EF_SF_EUIMID | 14 hex digits, §5.2.79 |
| `gid1`, `gid2` | EF_GID1, EF_GID2 | 32-bit, §5.2.72, §5.2.73 |
| `hrpdcap` | EF_HRPDCAP | `max_nai_length`, `max_ss_length`, `chap_supported`, §5.2.53 |
| `simple_ip` | EF_SIPUPP | list of `{index, nai, auth}`, §5.2.43 |
| `mobile_ip` | EF_MIPUPP | `retry` and a list of `nais`, §5.2.44 |
| `hrpd_access` | EF_HRPDUPP | `{nai, auth}`, §5.2.54 |
| `mms_auth_capability` | EF_AuthCapability | 16-bit, §5.2.69 |
| `a_key` | none | CAVE A-key, 16 hex or 20 decimal digits |
| `ssd` | none | SSD_A and SSD_B, 16 bytes hex |
| `aka_root_key`, `wlan_root_key` | none | 16 bytes hex |
| `ip_secrets` | none | list of `{class, index, secret_text}` or `{class, index, secret}` |
| `raw` | any | `{EF name: hex}` for EFs without an encoder |

Secrets (`a_key`, `ssd`, the root keys, `ip_secrets`, the PINs and the PUKs) are
stored on the card and cannot be read back.

## Packet data

`simple_ip`, `mobile_ip` and `hrpd_access` hold the NAIs the handset uses
for packet data. The first NAI in a list is the active one.

| Field | Values |
|---|---|
| `auth` in `simple_ip` | `none`, `chap` (default), `pap`, `chap_pap` |
| `auth` in `hrpd_access` | `none`, `chap` (default) |
| `mn_aaa_auth`, `mn_ha_auth` | `none`, `md5` (default) |
| `home_address`, `primary_home_agent`, `secondary_home_agent` | IPv4 address, default `0.0.0.0` |
| `retry` | `max_retries` 0-3, `first_retry_timeout_ms` 250-2000 in steps of 250, `rereg_threshold_min` 0-63 |

`ip_secrets` classes are `sip_chap`, `mn_ha`, `mn_aaa` and `hrpd`. The
index (0-3) must match the index of the NAI the secret belongs to, so only
NAIs 0-3 can have a secret. A secret is up to 32 bytes, given as text in
`secret_text` or as hex in `secret`.

## Services added automatically

Some keys add their service to `services`:

| Key | Service |
|---|---|
| `spn` | n10 |
| `simple_ip` | n14 |
| `mobile_ip` | n15 |
| `hrpd_access` | n8 |
| `mms_auth_capability` | n22 |
| `gid1`, `gid2` | n23, n24 |
| `sf_euimid` | n34 |

n32 (EF_EST) is always added. Do not list n1, n2, n30 or n31: the applet
has no phone book, FDN or multimedia files.

## Templates

`csim profile` fills a profile template for one IMSI and writes the
profile with mode 0600. `sim/examples/card.tmpl.yaml` is a template of
`card.yaml`.

```sh
csim profile sim/examples/card.tmpl.yaml --imsi 001010000000042 --mdn 5550142
```

| Placeholder | Value |
|---|---|
| `{imsi}` | `--imsi` |
| `{min}` | the last 10 digits of the IMSI |
| `{mdn}` | `--mdn`. Default: the MIN |
| `{luhn}` | the Luhn check digit of the digits before it |
| `{a_key}` | a random A-key |
| `{secret}` | a random 16-byte secret, new at each use |

Placeholders in comment lines are not filled. The output defaults to
`card-<IMSI>.local.yaml` next to the template, which Git ignores. An
existing output is replaced only with `--force`. Relative `prl_file` and
`eprl_file` paths are rewritten for the output's directory.
