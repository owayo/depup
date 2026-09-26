# エコシステムとモノレポ

モノレポで depup がどのディレクトリを処理するかと、エコシステムごとにどの宣言を読み取って書き換えるかをまとめた参照用のページです。

## モノレポ対応

追加で処理するディレクトリは [`.depup` ファイル](configuration.ja.md#depup-設定ファイル)に列挙します。次のワークスペースやプロジェクト構成は、depup が自動で検出します。

### pnpm ワークスペース

depup は `pnpm-workspace.yaml` を検出し、すべてのワークスペースパッケージを処理します。`packages` 配列はブロック形式（`- 'packages/*'`）とフロー形式（`packages: ['packages/*', 'apps/*']`）の両方に対応し、否定パターン（`!packages/legacy`）も扱えます。

### Cargo ワークスペース

`[workspace] members`（`crates/*` のような glob パターンを含む）を展開し、`[workspace] exclude` に挙げられたメンバーは処理しません。

### Go ワークスペース

`go.work` の `use` ディレクティブ（単一行形式と `use ( ... )` ブロック形式の両方）を展開し、各メンバーモジュールの `go.mod` を処理します。展開しないと、ルートに `go.mod` がない構成では、各メンバーに古い依存があっても「更新なし」と報告してしまいます。

### Gradle マルチプロジェクト

`settings.gradle` / `settings.gradle.kts` の `include ':app', ':core'`（Groovy 形式と Kotlin DSL 形式の両方）を展開し、各サブプロジェクトの `build.gradle` / `build.gradle.kts` と `buildSrc/` を処理します。依存宣言の大半はサブプロジェクト側にあるため、ルートのビルドファイルだけを見ていると取りこぼします。

`go.work` や `settings.gradle` から展開したパスには、`.depup` と同じ安全性の検査を適用します。絶対パス、`..` による親ディレクトリの参照、プロジェクトの外へ解決されるシンボリックリンクは拒否します。

### Tauri プロジェクト

depup は Tauri プロジェクトの `src-tauri/Cargo.toml` を自動検出します。

#### Tauri バージョン同期

Tauri プロジェクトでは、npm の `@tauri-apps/api` と Rust の `tauri` クレートのメジャー・マイナーバージョンが一致している必要があります。depup はこれらのバージョンを自動的に同期し、ビルドエラーを防ぎます。

```text
# エラー例（バージョン不一致）
Found version mismatched Tauri packages:
  tauri (v2.10.1) : @tauri-apps/api (v2.9.1)

# depup が自動的にバージョンを同期
@tauri-apps/api: 2.9.1 → 2.10.0
tauri: 2.9.0 → 2.10.1
```

両方のパッケージが同じメジャー・マイナーバージョン（例: `2.10.x`）になるよう、自動で調整されます。

## エコシステム別の詳細

この章は参照用です。depup がどの宣言を読み、どう書き換えるかを正確に知りたいときに、使っているエコシステムの節を引いてください。すべてのエコシステムに共通する規則は「[バージョン指定と書き換え](usage.ja.md#バージョン指定と書き換え)」にあります。

`package.json`（Bun catalogs を含む）と `composer.json` は、セクション名やパッケージ名が JSON エスケープで書かれていても更新できます。書き換えるのは依存セクション直下の文字列値だけで、ネストしたオブジェクトや配列は変更しません。元のキー表記、空白、改行コードは保持します。

### Node.js

`package.json` で更新するのは `dependencies`、`devDependencies`、`peerDependencies`、`optionalDependencies` です。`overrides` などのセクションは変更しません。

チルダの従来形式 `~>1.2.3`（node-semver 互換）も受け付け、更新後も `~>` を保持します。

npm の `=1.2` や `=1` のように、セグメントの一部だけを書いた比較（partial comparator）は固定指定ではありません。node-semver では `=1.2` が「1.2.x のどれでもよい」（`>=1.2.0 <1.3.0` と同じ）という意味になるためです。depup は `=` 演算子と、書かれているセグメント数を保って更新します（`=1.2` → `=2.3`、`=1` → `=2`）。

npm の comparator set（空白で区切った比較演算子の組）では、`1.2 <2.0.0` の `1.2` のように演算子を付けずに書いた部分バージョンも下限として扱い、その形を保ったまま更新します（`1.2 <2.0.0` → `1.9 <2.0.0`）。

node-semver の文法ではハイフンレンジの両端にワイルドカードも書けるため、`1.x - 2.x` のような形も解析・更新します。ただし depup が他の箇所で受け付けない端点（`1.x.3` のようにワイルドカードの後ろに数値が続く形式や、完全な浮動指定の `*`）は、ここでも受け付けません。

npm では、更新対象として解析する前に、バージョン文字列のプレリリース識別子とビルドメタデータ識別子を検証します。アンダースコアを含む識別子（`1.2.3-rc_1`）、空の識別子を含む形式（`1.2.3-alpha..1`）、先頭がゼロの数値プレリリース識別子（`1.2.3-01`）は対象外にします。書き換えると、`package.json` に不正な制約を書き出してしまうためです。

先頭ゼロの検証は、ビルドメタデータを切り落としてから行います。SemVer ではビルド識別子にハイフンや先頭ゼロを含められるため、`1.0.0+2024-01` や `1.2.3+00` のようなバージョンは有効なものとして更新対象に残します（検証するのはプレリリース部分だけです）。

Bun のワークスペースでは、ルートの `package.json` にあるトップレベルの `catalog` / `catalogs` と、`workspaces.catalog` / `workspaces.catalogs` を解析・更新します。ワークスペース内パッケージの `"react": "catalog:"` や `"jest": "catalog:testing"` のような参照は `catalog:` のまま残し、共有の catalog 定義側のバージョンだけを更新します。`pnpm-workspace.yaml` で定義する pnpm の catalogs は、まだマニフェストとして解析していません。pnpm の catalogs を参照する `package.json` の `catalog:` は、安全のため対象外にします。

### Python

PEP 621 / PEP 735 / Poetry に加えて、uv の旧形式 `[tool.uv] dev-dependencies` と PDM の `[tool.pdm.dev-dependencies]` も読み取ります。`[tool.uv.sources]` で PyPI 以外を指す依存（`workspace = true` / `git` / `path` / `url` / `pypi` 以外の `index`）は、ワークスペースのメンバーやカスタムインデックスの依存を PyPI の同名パッケージで上書きしないよう、対象外にします。Poetry の複数行の依存テーブル（`[tool.poetry.dependencies.<name>]` / `[tool.poetry.group.<g>.dependencies.<name>]`）と、TOML の引用符付きキー（`"zope.interface"` / `"ruamel.yaml"`。ドットを含む名前は、TOML では引用符で囲む必要があります）も解析・更新します。

`[project]` / `[tool.rye]` / `[tool.uv]` セクションで書き換えるのは `dependencies` / `dev-dependencies` 配列だけで、`name` / `description` / `keywords` などのメタデータ文字列は、PEP 508 の依存指定に見えても書き換えません。PEP 508 のバージョン指定は `>=3.5,<4,` のような末尾カンマを許しており、depup は下限を更新するときもこのカンマを残します。Poetry で `pypi` 以外の `source` を指定した依存も、depup が PyPI しか問い合わせないため対象外です。PEP 621 の依存を `tool.poetry.dependencies` で補足している場合も同じです。Poetry の複数制約の配列形式（`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`）も対象外です。depup は配列の要素ごとに `requires_python` を判定しないため、どの要素を書き換えればよいかを安全に決められません。

PyPI 以外をデフォルトのインデックスにしている `pyproject.toml` では、すべての依存を対象外にし、警告を 1 回表示します。該当するのは、Poetry の `priority = "primary"` / `"default"` の source、uv の `[[tool.uv.index]] default = true` や `[tool.uv] index-url`、PDM による `pypi` の上書きです。depup は PyPI しか問い合わせないため、更新すると非公開パッケージが同名の公開パッケージに置き換わります。

Python の互換リリース指定（compatible release、`~=`）は PEP 440 に従います。`~=1.2`（= `>=1.2,<2.0`）と `~=1.2.3`（= `>=1.2.3,<1.3.0`）は上限のある範囲として扱い、その範囲内で更新します。`~=1.2.3` は 1.2 系、`~=1.2` は 1.x 系の中に留まります。書き換えでは元のセグメント数を維持します（`~=1.2` → `~=1.9`。`~=1.9.0` にすると上限が `<1.10.0` に狭まるため）。単一セグメントの `~=1` は無効な形式なので対象外です。

PEP 440 の前方一致（prefix matching）は、`==1.2.*` / `!=1.2.*` のように、リリース番号の末尾に `.*` を付けて `==` か `!=` と組み合わせた指定でだけ受け付けます。`>=1.0.*`、`~=1.0.*`、`==1.0a1.*`、`==1.0.post1.*`、`==1.0+local.*` のような無効な形式は、解析の時点で対象外にします。`===1.0.*` は PEP 440 の arbitrary equality（バージョンとして解釈せず、文字列として完全に一致させる指定）なので、前方一致ではなく固定指定として扱います。

Poetry の `[tool.poetry.dependencies]` では、演算子のないバージョン文字列（`requests = "2.28.0"`）は `==2.28.0` と同じ完全一致の指定です（Poetry 公式ドキュメントの "Exact requirements"）。depup は文字列形式でも inline table（`{ version = "1.26.0", optional = true }`）でも、これを固定指定として `pinned` でスキップします。`--include-pinned` を付けたときは、演算子を付けないまま新しいバージョンへ書き換えます（`4.2.1` → `5.0.0`）。演算子なしを完全一致とみなすのは Poetry の設定内だけです。pip や PEP 508 の依存指定では演算子が必須なので、演算子のないバージョンは受け付けません。

PEP 440 の local version（`+` 以降のラベル）は、SemVer のビルドメタデータとは別物として、Python 固有の意味で扱います。固定指定と除外指定は、local ラベルを保ったまま解析しますが、書き換えません（`torch==2.1.0+cu121`、`!=1.0+local1`）。新しいバージョンがあれば、`parse error: constraint cannot be updated safely` としてスキップします。local ラベルは、同じ公開バージョン（public version）から作られた別ビルド（`cu121` は CUDA 12.1 向け）を指します。公開バージョンだけを進めると、存在しないビルドを指してしまいます。候補の比較では、local version を同じ公開バージョンより新しいものとして扱い（`1.0+local > 1.0`）、local 部分どうしも PEP 440 に従って比較します（`1.0+1 > 1.0+abc`、`1.0+abc.2 > 1.0+abc.1`）。PEP 440 が local ラベルを許していない順序比較・互換リリースの指定（`>=1.0+local`、`~=1.0+local`、`>=1.0+local,<2.0`）は対象外です。

PEP 440 のプレリリースは、区切り文字なしで書かれた場合（例: `2.0.0rc1`、`1.0rc1`、`1.0.0a1`）でも検出し、デフォルトで候補から外します。安定版を使っている依存が、rc 版へ誤って更新されることはありません。ポストリリース（`1.0.post1`）は対応する安定版より新しいものとして比較し、エポック（`1!2.3`）は比較で最優先します。プレリリースに付いたポストリリース（`1.0a1.post1`）も元のプレリリースより新しいものとして扱う（`1.0a1 < 1.0a1.post1 < 1.0`）ため、アルファ版を追っているユーザーもポストリリースの修正を取りこぼしません。

### Rust

`Cargo.toml` で書き換えるのは、`[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、`[workspace.dependencies]`、ターゲット固有の依存テーブルだけで、メタデータのテーブルは変更しません。

`alias = { package = "actual-crate", version = "1" }` のような Cargo のリネーム依存は、実際のパッケージ名でバージョンを取得し、マニフェスト上のキーへ書き戻します。`--only` / `--exclude` はどちらの名前でも指定できます。

path 依存（`{ path = "../common" }`）は、公開用に `version` を併記していても対象外です。ローカルのクレートで解決されるためです。crates.io 以外のレジストリを指す依存（`crates-io` 以外の `registry = "..."`、または `registry-index = "..."`）も、depup が crates.io しか問い合わせないため対象外です。

git 依存は、レジストリの代わりに `git ls-remote` で確認します。

| 指定 | 動作 |
|------|------|
| `tag` | 最新の安定版の SemVer タグへ書き換える。そのタグが `--max-change` の上限を超える場合は `max-change=<LEVEL>` としてスキップし、上限内の古いタグは候補にしない |
| `branch`、または指定なし（デフォルトブランチ） | リモートの先頭のコミットが `Cargo.lock` の記録と異なるとき、または記録がないときに、更新として報告する。`Cargo.toml` は書き換えず、新しいコミットは `--install` で実行する `cargo update` がロックファイルに取り込む。`--install` を付けなければ何も書き込まない |
| `rev` | `--include-pinned` を付けても、常に `pinned` としてスキップする |

`tag` の書き換えは、バージョンの書き換えと同じ依存テーブルと `[patch.<registry>]` / `[patch.<registry>.<package>]` に限り、inline table でも複数行テーブルでも単一引用符・二重引用符を保ちます。git 依存には age フィルターも OSV チェックも適用しません。`git ls-remote` に失敗した依存はスキップするだけで、終了コードは変えません。

Cargo の比較演算子による範囲指定では、`>=1.0, <2.0, >=1.0.100` のように 3 個以上の要件をカンマで区切った形式にも対応します。`^1.2.2, <1.5` のようにキャレット・チルダ・ワイルドカードと比較演算子を混ぜた複数要件も、`semver::VersionReq` で有効性を確かめたうえで範囲指定として検出します。上限がなく複数の下限が混在する制約（`>=1.2.3, ^1.3` など）は安全に書き換えられないため、スキップします。

### Go

`go.mod` には `^` や `~` のような範囲指定がなく、完全なバージョンしか書けません。そのため Go では、バージョンの書き方だけでは意図的な固定と判断しません。`// pinned` コメントのない Go の依存は、`--include-pinned` の有無にかかわらず常に更新対象です。更新したくない依存には、行末に `// pinned` コメントを付けてください。付けた依存は `pinned` としてスキップし、`--include-pinned` を指定したときだけ更新します。`// indirect; pinned` のように、コメント内のどこに `pinned` があっても認識します。

Go の `exclude` ディレクティブに書かれたバージョンは、候補から外します（`exclude` の記述自体は書き換えません）。上流モジュールの最新の `go.mod` が `retract` したバージョン（単一のバージョンでも閉区間でも）も候補から外します。タグ付きのバージョンがないモジュールでは Go Proxy の `@latest` へフォールバックします。`.info` の `Time` が省略されている場合は Unix エポックを公開日時として扱うため、公開日が不明なバージョンが `--age` でいつまでも候補から外れることはありません。

Go の `+incompatible` 付きバージョンは、候補に残すかどうかを `go` コマンドと同じ規則で決めます。判定の基準は、SemVer の順で最初に現れる `+incompatible` の直前にある compatible なバージョンです。そのバージョンに本物の `go.mod`（Go Proxy が合成したものではないもの）があれば、それ以降の `+incompatible` をすべて候補から外します。この処理がないと、`github.com/libp2p/go-libp2p` が `v0.49.0` から 2018 年の `v6.0.23+incompatible` へ「更新」され、しかもビルドは通るため、誤りに気づけません。

`replace` ディレクティブで置き換えているモジュールは対象外です。`require` だけを更新すると `replace` が一致しなくなり、ローカルのパッチが気づかないうちに外れるためです。バージョンなしの `replace` はそのモジュールのすべての `require` を、バージョン付きの `replace` は同じバージョンの `require` だけを対象外にします。

`go.mod` では、`) // direct deps` のようにコメントが付いたブロックの終端も通常の終端として扱い、`require` / `replace` / `exclude` ブロックの解析と更新に反映します。`require "golang.org/x/text" "v0.14.0"` のように引用符で囲まれたモジュールパスやバージョンも解析し、引用符を保ったまま更新します。単一行・ブロック形式の `require` を更新するときは、元の改行コード（LF / CRLF）を維持します。

### Ruby

Gemfile の依存宣言は、一般的な Ruby DSL 形式（`gem "rack", "~> 3.0"`）と括弧付きのメソッド呼び出し形式（`gem("rack", "~> 3.0")`）のどちらも解析・更新できます。更新時は元の呼び出し形式を保持します。同じ gem が複数箇所（例: トップレベルと `group :test` ブロックの両方）で宣言されている場合は、書き込み先が曖昧なので書き換えを拒否します。

`gem "pg", ">= 0.18", "< 2.0"` のような Gemfile の複合制約も解析・更新できます。進めるのは包含的な下限（`>= 0.18` のように値そのものを含む下限）だけで、元の引数の個数・順序・引用符の種類・空白・括弧付きの呼び出し形式・行末の `if` / `unless` 修飾子を保ったまま書き戻します。比較の基準は記述順に関係なく包含的な下限なので、`gem "pg", "< 2.0", ">= 0.18"` でも `0.18` を基準に判定します。書き換え後の制約を元の引数の個数へ分割できない場合（引数自体がカンマを含む場合など）は、安全でない書き換えはせずにエラーとして報告します。`gem "rack", "!= 2.2.4"` のような除外制約は、一部だけを更新すると意味が変わるため、スキップします。

Gemfile の次の宣言は対象外です。

- バージョンを書かずに `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` を指定した gem。RubyGems のレジストリ依存ではないためです。オプションキーは、hash-rocket 形式（`:git => '...'`）も同じように認識します。
- `git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック内の gem。理由は同じです。
- 引数が次の行へ続く宣言（`gem "devise",`）。その行だけではバージョンを決められないためです。

`git:` などを指定していてもバージョンが明示されていれば、Bundler が gemspec のバージョンを検証するための制約として、source オプションを保ったまま解析・更新します。`platforms` / `install_if` のような通常のブロック内の gem は、ほかの gem と同じく処理します。行単位の `group:` / `groups:` オプションは、開発依存かどうかの判定に使います。

Bundler の `git_source(:name) { ... }` で登録したカスタムの git source ショートハンド（例: `gem 'rails', stash: 'forks/rails'`）も、組み込みの `git:` / `github:` と同じ規則で扱います。バージョンのない宣言は、レジストリ外の依存として対象外にします。

### PHP

`composer.json` で更新するのは `require` と `require-dev` です。`replace` / `provide` / `conflict` などのセクションは変更しません。明示的な等価演算子（`=1.2.3` / `==1.2.3`）は、演算子を保ったまま更新します。「等しくない」を `<>` で書いた除外制約（`<>1.2.3` など）は、解析しますが書き換えず、スキップします（[自動更新しない制約](usage.ja.md#自動更新しない制約)）。

Composer のプラットフォームパッケージ（`php`、`hhvm`、`ext-*`、`lib-*`、Composer API パッケージなど）は対象外です。`1.0.0 as 1.1.0` のようなインラインエイリアスも対象外です。レジストリの最新版で上書きすると、エイリアスの宣言が壊れてしまうためです。

Composer と Packagist は、`composer/semver` の `VersionParser` に従って 1〜4 セグメントの数値バージョンを有効とみなします。depup も `1.2.3.4`、`^1.0.0.0`、`~3.4.5.6`、`1.0.0.*` のような 4 セグメントまでのバージョンを解析・更新できます。5 セグメント以上は無効なので対象外です。

Composer の修飾子（modifier）は区切り文字を省略でき、`.` / `_` も区切りに使えます（`composer/semver` の正規表現は `[._-]?`）。そのため depup は、`5.0.0alpha3` / `1.0.0.RC1` / `1.0.0_beta1` をプレリリース、`2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` を元のバージョンより新しい修正版（patch alias）として扱います。どちらの形式も現在の Packagist に実在します（`nikic/php-parser` の `5.0.0beta1`、`laminas/laminas-diactoros` のセキュリティパッチ `2.2.1p2`）。

Composer は `~>` 演算子を受け付けない（`Invalid operator "~>"`）ため、PHP では `~>` を含む制約を対象外にし、Composer が読めない制約を書き戻さないようにしています。node-semver は `~>` を有効とするので、Node.js では受け付けます。

### Java（Gradle）

`strictly` / `require` / `prefer` / `reject` を使う Gradle の rich version 宣言は、`implementation("org.slf4j:slf4j-api") { version { ... } }` のような依存ブロック内でも解析します。文字列記法の短縮形でも、完全一致（`group:name:1.2.3!!`）、動的プレフィックス（`group:name:5.3.+!!`）、範囲（`group:name:[1.7, 1.8[!!`）、prefer 付きの strict 範囲（`group:name:[1.7, 1.8[!!1.7.25`）を解析できます。`strictly` または `require` で範囲を、`prefer` で優先するバージョンを指定している場合、depup は範囲を上限制約として維持したまま `prefer` の値を更新します。`reject` に列挙されたバージョンは候補から外し、`2.+` のような動的な reject や `[1.5,1.9)` のような範囲の reject も考慮します。

Gradle の宣言ラッパー `platform(...)` / `enforcedPlatform(...)` / `testFixtures(...)` にも対応しています。`implementation platform('com.google.cloud:libraries-bom:26.1.0')` や `testImplementation(platform("org.junit:junit-bom:5.10.0"))` のような BOM 宣言も解析・更新でき、開発依存かどうかはラッパーの外側にある configuration 名（`implementation` / `testImplementation` など）で判定します。

`ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入も `ext { ... }` ブロックと同じく変数として解決し、`${Versions.retrofit}` のような修飾付きの参照は最後のセグメントで解決します。同じ変数名が異なる値で複数定義されている場合は変数を解決せず、その変数を参照する依存は対象外になります。別オブジェクトの値を拾う誤更新を避けるためです。

`gradle/*.versions.toml` にある Gradle version catalog は、Java のマニフェストとして検出します。depup は `[libraries]` の `alias = "group:name:version"`、`module = "group:name"`、`group` / `name` / `version`、`version.ref` を解析し、参照先の `[versions]` もその場で更新します。`strictly` / `require` / `prefer` / `reject` / `rejectAll` を含む rich version table は、Gradle のビルドファイルと同じ規則で候補を選びます。`[plugins]` に並ぶのは Gradle のプラグイン ID で、Maven Central の座標とは一致しないため、対象外です。

Gradle の文字列記法では、`:resources@zip` や `@aar` のような classifier / extension の接尾辞を保持します。`//` の行コメントや `/* ... */` のブロックコメントの中だけにある依存宣言は無視します。Gradle version catalog では、バージョンが宣言されている TOML の文字列形式またはテーブル形式を保ったまま更新します。

`-SNAPSHOT` / `.SNAPSHOT` で終わるバージョンを直接指定した依存（`1.2.3-SNAPSHOT`、`1.2.3-SNAPSHOT!!`、`[1.2.3-SNAPSHOT]`）は対象外です。SNAPSHOT は、依存を解決するたびに最新のタイムスタンプ付きビルドを取り直す「動く参照」です。固定のリリース版へ書き換えると、ビルドに使われる中身が気づかないうちに変わります。`.Final` / `.RELEASE` / `-jre` / `-SP1` のような安定版の qualifier が付いたバージョンは、通常どおり更新します。

Gradle の `resolutionStrategy { force ... }` / `constraints { }` / `dependencySubstitution { }` の中に書かれた座標は対象外です。これらのブロックは、他の場所で宣言済みのバージョンを再掲するものです。別々の宣言として扱うと同じ座標が重複し、書き込み先が曖昧になって更新できなくなります。

JVM 系の milestone 版はプレリリースとして扱い、現在のバージョンが安定版なら候補から外します。対象は `4.0.0-M1`、旧 Spring Boot のドット区切り `2.0.0.M1`、省略しない綴りの `-milestone1` です。この扱いがないと、`assertj-core 3.24.2` が `4.0.0-M1` へ、`junit-bom 5.10.0` が `5.13.0-M3` へ、`spring-core 5.3.23` が `7.0.0-M6` へ更新されてしまいます。判定は「`m` の直後が数字」のトークンに限るため、JVM の安定版の qualifier（`.Final`、`-jre`、`-android`、`.RELEASE`、`.GA`、`-SP1`）や `-macos1` のような識別子を誤判定しません。他のプレリリースと同じく、現在のバージョンが milestone の場合は候補に milestone を残すため、次の milestone へ進めます。

### Swift

Swift で対象になるのは、GitHub の URL で指定し、バージョンの要件を書いた依存だけです。GitHub 以外の URL と `branch:` / `revision:` の指定は対象外です。Swift Package Registry の `id:` 依存（`.package(id: "scope.name", ...)`）も、registry API のアダプターが未実装のため対象外です（将来対応する予定です）。

- URL は、HTTPS、scp 形式の SSH（`git@github.com:owner/repo.git`）、標準の SSH（`ssh://git@github.com/owner/repo.git`）、GitHub の SSH over 443（`ssh://git@ssh.github.com:443/owner/repo.git`）を解析します。
- GitHub のタグは `v1.2.3` と `V1.2.3` の両方を認識します。`Package.swift` の version requirement 文字列は、厳格な SemVer（`X.Y.Z`、先頭ゼロなし）として検証します。
- SwiftPM が Semantic Versioning 2.0.0 に従うのに合わせて、プレリリース識別子（`1.0.0-beta.1`）、ビルドメタデータ（`1.0.0+build.123`）、その両方を含む形式（`1.0.0-rc.1+sha.abc`）も解析・更新します。
- `.package(...)` の末尾に `traits: [...]`（SPM 6.1 の Package Traits）や `moduleAliases: [...]` のような引数があっても、version requirement だけを置き換え、ほかの引数は保ちます。
- `//` の行コメントや `/* ... */` のブロックコメントの中にある依存宣言は無視します。

### mise

mise（[jdx/mise](https://mise.jdx.dev)）の設定ファイルに書かれたツールバージョンも、他の言語と同じワークフロー（検出 → 解析 → 判定 → 書き込み）で更新します。バージョン一覧は `mise ls-remote <tool> --json` から取得するため、mise が対応するすべてのバックエンド（core / aqua / ubi / asdf / npm: / cargo: / go: / pipx: など）をそのまま扱えます。

#### 対象ファイル

`mise.toml` / `.mise.toml` / `mise/config.toml` / `.mise/config.toml` / `.config/mise.toml` / `.config/mise/config.toml` / `.tool-versions`

`mise.local.toml`（個人のローカル上書き。通常は gitignore の対象）と `mise.<env>.toml`（環境別の上書き設定）は、特定の環境だけに効く設定です。更新すると、その環境の設定だけが利用者の気づかないうちに変わってしまうため、意図的に読み込みません。

#### バージョン指定の扱い

| 記法 | 例 | 扱い |
|------|-----|------|
| 完全一致 | `node = "26.7.0"` | 最新版へ更新 |
| 前方一致 | `node = "26"` / `"26.7"` | セグメント数を保って更新（`26` → `27`、`26.7` → `26.8`） |
| 明示セレクター | `go = "prefix:1.19"` | `prefix:` を保持して更新（`prefix:1.24`） |
| ベンダー付き | `java = "temurin-21.0.5"` | 同じベンダー内で更新（`temurin-21.0.9`） |
| inline table | `python = { version = "3.13", virtualenv = ".venv" }` | `version` だけ更新し、他のオプションは保持 |
| テーブル形式 | `[tools.terraform]` + `version = "1.15.0"` | 同上 |
| 浮動指定 | `latest` / `lts` / `system` | 固定のバージョンがなく、更新するものがないため対象外 |
| 非バージョン | `ref:master` / `path:./shfmt` / `sub-2:lts` | 対象外 |
| 複数バージョン | `python = ["3.12", "3.13"]` | どれを更新すべきか決められないため対象外 |

`mise ls-remote java` が返す 3000 件超の候補の大半は、`temurin-` / `graalvm-community-` / `zulu-` などのベンダー接頭辞付きです。depup は現在の指定と同じ接頭辞の候補だけを残すため、`temurin-21` を使っているプロジェクトが `zulu-27` に書き換わることはありません。

`[tools]` セクションだけを書き換え、`[settings]` / `[env]` / `[tasks]` / `[alias]` に同名のキーがあっても変更しません。引用符の種類（`"` / `'`）、行末コメント、改行コード（CRLF）、`.tool-versions` の空白の並びはすべて保持します。

#### mise と age フィルター

mise の `[settings] minimum_release_age` が**明示的に書かれている**場合は、pnpm・Bun の `minimumReleaseAge` と同じ[プロジェクトポリシー](usage.ja.md#適用される-age-の優先順位)として、CLI の `--age` より優先します。採用するのは設定ファイルに書かれた値だけで、mise の組み込みのデフォルト（24 時間）は採用しません。書き方は「[`minimumReleaseAge` を読み取る設定ファイル](usage.ja.md#minimumreleaseage-を読み取る設定ファイル)」を参照してください。

> **注意**: mise の `m` は**分**（humantime 準拠）で、depup CLI の `--age 1m`（1 か月）とは単位が違います。`minimum_release_age = "1m"` は 1 分として扱われます。プロジェクトポリシーは CLI の `--age` より優先されるため、pnpm や Bun にもっと大きな値がなければ、age フィルターは実質的に効かなくなります。1 か月にしたい場合は `"1M"` か `"30d"` と書いてください。

`mise ls-remote` はデフォルトで mise 側の `minimum_release_age` を適用して新しいバージョンを隠しますが、depup は `--minimum-release-age 0` を渡してすべて取得し、age の判定を depup 側に一本化します。`minimum_release_age_excludes` は depup では解釈しないため、設定されている場合は警告を表示します。
