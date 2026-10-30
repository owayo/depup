//! ネットワークとファイル I/O を除いたマニフェスト更新の計測。
//! `mise exec -- cargo run --release --locked --example manifest_update_bench -- 1000 7`
//! Cargo の各宣言形式は末尾に `cargo` を付けて選ぶ。

use depup::manifest::{CargoTomlParser, ComposerJsonParser, ManifestParser, PackageJsonParser};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

fn count_allocation(size: usize) {
    if COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
    }
}

// 単一スレッドの計測で要求された確保量を数える。実際の確保と解放は System に任せる。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn fixture(count: usize, section: &str, escaped: bool) -> (String, String) {
    let mut dependencies = serde_json::Map::new();
    let vendor = if section == "require" {
        "vendor"
    } else {
        "@vendor"
    };
    for index in 0..count {
        dependencies.insert(format!("{vendor}/pkg-{index:04}"), "^1.0.0".into());
    }
    let target = format!("{vendor}/pkg-{:04}", count - 1);
    let mut document = serde_json::json!({
        "name": "manifest-benchmark",
        "description": "日本語と { braces } と \"引用符\" を含むメタデータ",
        "overrides": { target.clone(): "^1.0.0" },
    });
    match section {
        "bun" => {
            document["workspaces"] = serde_json::json!({
                "packages": ["packages/*"],
                "catalogs": { "default": dependencies },
            })
        }
        _ => document[section] = dependencies.into(),
    }
    let mut content = serde_json::to_string_pretty(&document).unwrap();
    if escaped {
        // エスケープしたキーでも同じ依存を選び、元の綴りを保持する。
        content = content
            .replace("vendor/", "vendor\\u002f")
            .replace('\n', "\r\n");
    }
    (content, target)
}

fn positive_argument(index: usize, default: usize) -> usize {
    let value = std::env::args()
        .nth(index)
        .map(|value| value.parse::<usize>().expect("正の整数を指定してください"))
        .unwrap_or(default);
    assert!(value > 0, "正の整数を指定してください");
    value
}

fn measure(
    name: &str,
    iterations: usize,
    samples: usize,
    expected: &str,
    update: impl Fn() -> String,
) {
    for _ in 0..100 {
        black_box(update());
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let measured = update();
    COUNTING.store(false, Ordering::Relaxed);
    assert_eq!(measured, expected);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    for sample in 1..=samples {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(update());
        }
        let ns = started.elapsed().as_nanos() as f64 / iterations as f64;
        println!("{name},{sample},{iterations},{ns:.2},{allocations},{allocated_bytes}");
    }
}

fn cargo_cases(iterations: usize, samples: usize) {
    for (name, count, style, git) in [
        ("cargo-simple-1", 1, "simple", false),
        ("cargo-simple-100", 100, "simple", false),
        ("cargo-simple-1000", 1000, "simple", false),
        ("cargo-inline-100", 100, "inline", false),
        ("cargo-dotted-100", 100, "dotted", false),
        ("cargo-table-100", 100, "table", false),
        ("cargo-git-inline-100", 100, "inline", true),
        ("cargo-git-dotted-100", 100, "dotted", true),
        ("cargo-git-table-100", 100, "table", true),
    ] {
        let field = if git { "tag" } else { "version" };
        let old = if git { "v1.0.0" } else { "^1.0.0" };
        let new = if git { "v2.3.4" } else { "^2.3.4" };
        let target = format!("crate-{:04}", count - 1);
        let mut content = String::from("[dependencies]\r\n");
        let mut target_line = String::new();
        for index in 0..count {
            let package = format!("crate-{index:04}");
            let line = match style {
                "simple" => format!("{package} = '{old}'\r\n"),
                "inline" => {
                    let source = if git {
                        "git = 'https://example.com/repo', "
                    } else {
                        ""
                    };
                    format!("{package} = {{ {source}{field} = '{old}', features = ['std'] }}\r\n")
                }
                "dotted" => {
                    let source = if git {
                        format!("{package}.git = 'https://example.com/repo'\r\n")
                    } else {
                        String::new()
                    };
                    format!("{source}{package}.{field} = '{old}'\r\n")
                }
                "table" => {
                    let source = if git {
                        "git = 'https://example.com/repo'\r\n"
                    } else {
                        ""
                    };
                    format!(
                        "[dependencies.{package}]\r\n{source}{field} = '{old}'\r\nfeatures = ['std']\r\n"
                    )
                }
                _ => unreachable!(),
            };
            content.push_str(&line);
            if package == target {
                target_line = line;
            }
        }
        // 同名のメタデータと最終行の改行なしも、出力の完全一致で検証する。
        content.push_str(&format!("[package.metadata]\r\n{target} = '{old}'"));
        let expected = content.replacen(&target_line, &target_line.replace(old, new), 1);
        toml::from_str::<toml::Value>(&content).unwrap();
        let update = || {
            if git {
                CargoTomlParser.update_git_tag(black_box(&content), black_box(&target), "v2.3.4")
            } else {
                CargoTomlParser.update_version(black_box(&content), black_box(&target), "2.3.4")
            }
            .unwrap()
        };
        assert_eq!(update(), expected);
        toml::from_str::<toml::Value>(&expected).unwrap();
        measure(name, iterations, samples, &expected, update);
    }
}

fn main() {
    let iterations = positive_argument(1, 1000);
    let samples = positive_argument(2, 7);
    println!("case,sample,iterations,ns_per_update,allocations,allocated_bytes");
    if std::env::args().nth(3).as_deref() == Some("cargo") {
        cargo_cases(iterations, samples);
        return;
    }
    for (name, count, section, escaped) in [
        ("node-1", 1, "dependencies", false),
        ("node-100", 100, "dependencies", false),
        ("node-1000", 1000, "dependencies", false),
        ("node-escaped-100", 100, "dependencies", true),
        ("bun-100", 100, "bun", false),
        ("composer-1000", 1000, "require", false),
    ] {
        let parser: &dyn ManifestParser = if section == "require" {
            &ComposerJsonParser
        } else {
            &PackageJsonParser
        };
        let (content, target) = fixture(count, section, escaped);
        let old_json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let mut expected_json = old_json.clone();
        if section == "bun" {
            expected_json["workspaces"]["catalogs"]["default"][&target] = "^2.3.4".into();
        } else {
            expected_json[section][&target] = "^2.3.4".into();
        }
        let expected = parser.update_version(&content, &target, "2.3.4").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&expected).unwrap(),
            expected_json
        );
        if escaped {
            assert!(expected.contains("vendor\\u002f"));
            assert_eq!(
                expected.matches("\r\n").count(),
                content.matches("\r\n").count()
            );
        }
        let update = || {
            parser
                .update_version(black_box(&content), black_box(&target), "2.3.4")
                .unwrap()
        };
        measure(name, iterations, samples, &expected, update);
    }
}
