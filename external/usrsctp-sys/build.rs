use std::env;
use std::path::PathBuf;

const CORE_SOURCES: &[&str] = &[
    "user_environment.c",
    "user_mbuf.c",
    "user_recv_thread.c",
    "user_socket.c",
    "netinet/sctp_asconf.c",
    "netinet/sctp_auth.c",
    "netinet/sctp_bsd_addr.c",
    "netinet/sctp_callout.c",
    "netinet/sctp_cc_functions.c",
    "netinet/sctp_crc32.c",
    "netinet/sctp_indata.c",
    "netinet/sctp_input.c",
    "netinet/sctp_output.c",
    "netinet/sctp_pcb.c",
    "netinet/sctp_peeloff.c",
    "netinet/sctp_sha1.c",
    "netinet/sctp_ss_functions.c",
    "netinet/sctp_sysctl.c",
    "netinet/sctp_timer.c",
    "netinet/sctp_userspace.c",
    "netinet/sctp_usrreq.c",
    "netinet/sctputil.c",
    "netinet6/sctp6_usrreq.c",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let lib_dir = manifest_dir.join("../usrsctp/usrsctplib");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("set by cargo");

    println!("cargo:rerun-if-changed={}", lib_dir.display());

    let mut build = cc::Build::new();
    build
        .include(&lib_dir)
        .include(lib_dir.join("netinet"))
        .include(lib_dir.join("netinet6"))
        .std("c99")
        .warnings(false)
        .define("__Userspace__", None)
        .define("SCTP_SIMPLE_ALLOCATOR", None)
        .define("SCTP_PROCESS_LEVEL_LOCKS", None)
        .define("INET", None)
        .define("INET6", None)
        .define("HAVE_SYS_QUEUE_H", None)
        .define("HAVE_NETINET_IP_ICMP_H", None)
        .define("HAVE_STDATOMIC_H", None);

    match target_os.as_str() {
        "linux" | "android" => {
            build
                .define("_GNU_SOURCE", None)
                .define("HAVE_LINUX_IF_ADDR_H", None)
                .define("HAVE_LINUX_RTNETLINK_H", None);
        }
        "macos" | "ios" => {
            build
                .define("__APPLE_USE_RFC_2292", None)
                .define("HAVE_SA_LEN", None)
                .define("HAVE_SIN_LEN", None)
                .define("HAVE_SIN6_LEN", None)
                .define("HAVE_SCONN_LEN", None)
                .flag_if_supported("-Wno-deprecated-declarations");
        }
        "freebsd" => {
            build
                .define("HAVE_SA_LEN", None)
                .define("HAVE_SIN_LEN", None)
                .define("HAVE_SIN6_LEN", None)
                .define("HAVE_SCONN_LEN", None);
        }
        other => panic!("usrsctp-sys does not support target_os={other}"),
    }

    for source in CORE_SOURCES {
        build.file(lib_dir.join(source));
    }
    build.compile("usrsctp");

    if target_os != "macos" && target_os != "ios" {
        println!("cargo:rustc-link-lib=pthread");
    }
}
