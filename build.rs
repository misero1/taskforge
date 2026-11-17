//! Embeds Windows resources into the executable: the app icon, the program
//! details shown in File Properties > Details (publisher, description,
//! version), and an application manifest.
//!
//! `assets/app.res` holds the compiled icon (regenerate it with
//! `python tools/make_icon.py`). This script appends a version resource and
//! the manifest, using the version from Cargo.toml, and links the result.
//! MSVC's linker and LLVM's lld accept .res files directly. GNU ld doesn't, so
//! for `*-windows-gnu` builds it is linked only when the linker is lld-based
//! (set `TASKFORGE_LINK_RES=1`, e.g. with cargo-zigbuild).

use std::path::PathBuf;

const COMPANY: &str = "TaskForge";
const PRODUCT: &str = "TaskForge";
const DESCRIPTION: &str = "TaskForge - Task Manager for Windows";
const COPYRIGHT: &str = "Copyright (c) 2026 TaskForge contributors. MIT License.";

fn main() {
    println!("cargo:rerun-if-changed=assets/app.res");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TASKFORGE_LINK_RES");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let forced = std::env::var("TASKFORGE_LINK_RES").is_ok_and(|v| v == "1");
    if os != "windows" || !(env == "msvc" || forced) {
        return;
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut res = std::fs::read(root.join("assets").join("app.res")).expect("assets/app.res");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let nums = version_numbers(&version);
    append_resource(&mut res, 16, 1, &version_info(&version, nums)); // RT_VERSION
    append_resource(&mut res, 24, 1, manifest(nums).as_bytes()); // RT_MANIFEST
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("taskforge.res");
    std::fs::write(&out, res).expect("write .res");
    println!("cargo:rustc-link-arg-bins={}", out.display());
    if env == "msvc" {
        // The manifest is embedded above; don't let link.exe generate another.
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
}

fn version_numbers(v: &str) -> [u16; 4] {
    let mut n = [0u16; 4];
    for (i, part) in v.split(['.', '-', '+']).take(3).enumerate() {
        n[i] = part.parse().unwrap_or(0);
    }
    n
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Append one entry to a .res file (type and name given as numeric ids).
fn append_resource(res: &mut Vec<u8>, ty: u16, name: u16, data: &[u8]) {
    pad4(res);
    let mut hdr = Vec::new();
    hdr.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    hdr.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    hdr.extend_from_slice(&[0xFF, 0xFF]);
    hdr.extend_from_slice(&ty.to_le_bytes());
    hdr.extend_from_slice(&[0xFF, 0xFF]);
    hdr.extend_from_slice(&name.to_le_bytes());
    hdr.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    hdr.extend_from_slice(&0x0030u16.to_le_bytes()); // MemoryFlags: MOVEABLE | PURE
    hdr.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId: en-US
    hdr.extend_from_slice(&0u32.to_le_bytes()); // Version
    hdr.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    res.extend_from_slice(&hdr);
    res.extend_from_slice(data);
    pad4(res);
}

/// One node of a VS_VERSIONINFO tree: wLength, wValueLength, wType, key, value, children.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    b
}

fn string_entry(key: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    node(key, &v, (v.len() / 2) as u16, true, &[])
}

fn version_info(version: &str, n: [u16; 4]) -> Vec<u8> {
    let ms = ((n[0] as u32) << 16) | n[1] as u32;
    let ls = ((n[2] as u32) << 16) | n[3] as u32;
    let mut fixed = Vec::new();
    for v in [
        0xFEEF04BDu32, // signature
        0x0001_0000,   // structure version
        ms,
        ls, // file version
        ms,
        ls,          // product version
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&v.to_le_bytes());
    }
    let dotted = format!("{}.{}.{}.{}", n[0], n[1], n[2], n[3]);
    let strings = [
        string_entry("CompanyName", COMPANY),
        string_entry("FileDescription", DESCRIPTION),
        string_entry("FileVersion", &dotted),
        string_entry("InternalName", "TaskForge"),
        string_entry("LegalCopyright", COPYRIGHT),
        string_entry("OriginalFilename", "TaskForge.exe"),
        string_entry("ProductName", PRODUCT),
        string_entry("ProductVersion", version),
    ];
    let table = node("040904B0", &[], 0, true, &strings);
    let sfi = node("StringFileInfo", &[], 0, true, &[table]);
    let translation = 0x04B0_0409u32.to_le_bytes(); // en-US, Unicode
    let var = node("Translation", &translation, 4, false, &[]);
    let vfi = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[sfi, vfi],
    )
}

fn manifest(n: [u16; 4]) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="TaskForge.TaskForge" version="{}.{}.{}.{}"/>
  <description>{DESCRIPTION}</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#,
        n[0], n[1], n[2], n[3]
    )
}
