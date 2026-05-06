use std::path::PathBuf;

fn main() {
    let csrc = PathBuf::from("csrc");

    let mut build = cc::Build::new();

    // Include paths
    build
        .include(csrc.join("include"))
        .include(csrc.join("code"))
        .include(csrc.join("dsp_fx"))
        .include(csrc.join("dspmath"))
        .include(&csrc);

    // Suppress warnings from old C89/C99 vendored code
    build
        .warnings(false)
        .flag_if_supported("-Wno-implicit-function-declaration")
        .flag_if_supported("-Wno-implicit-int")
        .flag_if_supported("-Wno-incompatible-pointer-types")
        .flag_if_supported("-Wno-int-conversion")
        .flag_if_supported("-Wno-deprecated-non-prototype")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-sometimes-uninitialized")
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-dangling-else")
        .flag_if_supported("-Wno-sign-compare")
        .flag_if_supported("-Wno-absolute-value")
        .flag_if_supported("-Wno-unsequenced");

    // Core codec source files from code/ (skip main.c -- it has its own main())
    let code_sources = [
        "a2lsp.c",
        "acb_ex.c",
        "acelp_pf.c",
        "apf.c",
        "auto.c",
        "bitpack.c",
        "bitupack.c",
        "bl_intrp.c",
        "bqiir.c",
        "c3_10pf.c",
        "c8_35pf.c",
        "comacb.c",
        "convh.c",
        "cshift.c",
        "d_fer.c",
        "d_globs.c",
        "d_no_fer.c",
        "d_rate_1.c",
        "d3_10pf.c",
        "d8_35pf.c",
        "decode.c",
        "durbin.c",
        "e_globs.c",
        "encode.c",
        "fcbgq.c",
        "fer.c",
        "filter.c",
        "fndppf.c",
        "getext1k.c",
        "getgain.c",
        "getopt.c",
        "getres.c",
        "globs.c",
        "impulser.c",
        "interpol.c",
        "intr_cos.c",
        "inv_sqrt.c",
        "lpcana.c",
        "lsp2a.c",
        "lspmaq.c",
        "maxeloc.c",
        "mdfyorig.c",
        "mod.c",
        "ns127.c",
        "pit_shrp.c",
        "pktoav.c",
        "pre_enc.c",
        "putacbc.c",
        "r_fft.c",
        "rda.c",
        "rom.c",
        "synfltr.c",
        "w2res.c",
        "weight.c",
        "zeroinpt.c",
    ];
    for src in &code_sources {
        build.file(csrc.join("code").join(src));
    }

    // DSP math library
    let dspmath_sources = [
        "ehwutl.c",
        "globdefs.c",
        "mathadv.c",
        "mathdp31.c",
        "mathevrc.c",
    ];
    for src in &dspmath_sources {
        build.file(csrc.join("dspmath").join(src));
    }

    // DSP fixed-point operations
    build.file(csrc.join("dsp_fx").join("basic_op40.c"));

    // Wrapper files
    build.file(csrc.join("evrcc.c"));
    build.file(csrc.join("evrcpacket.c"));
    build.file(csrc.join("tty_stubs.c"));

    build.compile("evrcc");

    let evrc_b_wb = csrc.join("evrc_b_wb");
    let mut bw_build = cc::Build::new();
    bw_build
        .cpp(true)
        .include(&evrc_b_wb)
        .include(&csrc)
        .warnings(false)
        .flag_if_supported("-std=c++98")
        .flag_if_supported("-fvisibility=hidden")
        .flag_if_supported("-fvisibility-inlines-hidden")
        .flag_if_supported("-fpermissive")
        .flag_if_supported("-Wno-deprecated")
        .flag_if_supported("-Wno-deprecated-declarations")
        .flag_if_supported("-Wno-writable-strings")
        .flag_if_supported("-Wno-extra-qualification")
        .flag_if_supported("-Wno-register")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-sign-compare");

    for (symbol, prefixed) in [
        ("bq_w", "evrcbw_bq_w"),
        ("gnvq_4", "evrcbw_gnvq_4"),
        ("gnvq_8", "evrcbw_gnvq_8"),
        ("lognsize22", "evrcbw_lognsize22"),
        ("lognsize28", "evrcbw_lognsize28"),
        ("lognsize8", "evrcbw_lognsize8"),
        ("lsptab8", "evrcbw_lsptab8"),
        ("nsize22", "evrcbw_nsize22"),
        ("nsize28", "evrcbw_nsize28"),
        ("nsize8", "evrcbw_nsize8"),
        ("nsub22", "evrcbw_nsub22"),
        ("nsub28", "evrcbw_nsub28"),
        ("nsub8", "evrcbw_nsub8"),
        ("ppvq", "evrcbw_ppvq"),
        ("rnd_delay", "evrcbw_rnd_delay"),
    ] {
        bw_build.define(symbol, prefixed);
    }

    let evrc_b_wb_sources = [
        "dafpindex.cc",
        "io.cc",
        "rom.cc",
        "preproc.cc",
        "preproc1.cc",
        "encode.cc",
        "decode.cc",
        "filt.cc",
        "lpcana.cc",
        "lsp.cc",
        "olpitch.cc",
        "rcelp.cc",
        "acb_cmn.cc",
        "celp.cc",
        "acbevrc.cc",
        "interp.cc",
        "acelp.cc",
        "voiced.cc",
        "ppp.cc",
        "WI.cc",
        "nelp.cc",
        "uvgq.cc",
        "silence.cc",
        "WBSmartBlanking.cc",
        "lsp_cb2233_6697e.cc",
        "cb_8R_quant.cc",
        "lspq.cc",
        "packet.cc",
        "fer.cc",
        "genutils.cc",
        "pf.cc",
        "bad_rate.cc",
        "nacfap.cc",
        "rda.cc",
        "mode.cc",
        "new_mode.cc",
        "filterbank.cc",
        "filterbank_coef.cc",
        "cod3_10jcelp.cc",
        "dec3_10jcelp.cc",
        "wsnr.cc",
        "lsp_vq_28.cc",
        "lsp_vq_22.cc",
        "lsp_vq_16.cc",
        "lsp_vq_10.cc",
        "lsp_vq_8.cc",
        "cod7_35.cc",
        "hpf80.cc",
        "gen_lsp_weights.cc",
        "r_fft_float.cc",
        "UB_gain_cb.cc",
        "UB_lsf_cb.cc",
        "UB_analysis_windows.cc",
        "vectors.cc",
        "wideband_encoder_v1.cc",
        "gen_shaped_noisev2.cc",
        "WB_analysis_windows_8thRate.cc",
        "Quantize_WB_params.cc",
        "MSencode.cc",
        "NoiseSynthesis.cc",
        "declick_frame_v3.cc",
        "fwd_smooth.cc",
        "bck_smooth.cc",
        "log_envelope_1KHz.cc",
        "wideband_encoder_8thRate.cc",
        "WB_Erasure_processing.cc",
        "fgv_mem.cc",
        "ER_BWext.cc",
        "UB_enc.cc",
        "WB_encoder.cc",
        "WB_decoder.cc",
        "interppitchfilt.cc",
        "hnwfilters.cc",
        "pitchprefilter.cc",
        "music_mode.cc",
        "celp_mdct_discriminator.cc",
        "blind_bg.cc",
        "fplib_def.cc",
        "bigint.cc",
        "mdct.cc",
    ];
    for src in &evrc_b_wb_sources {
        bw_build.file(evrc_b_wb.join(src));
    }
    bw_build.file(csrc.join("evrcbw.cc"));
    bw_build.compile("evrcbw");

    // Rerun if any C source or header changes
    println!("cargo:rerun-if-changed=csrc/");
}
