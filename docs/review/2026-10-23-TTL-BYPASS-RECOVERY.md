# TTL・緊急バイパス・WAL復旧の修正記録

対象は `7e9473c` からの作業ツリー差分。[前回の修正](2026-10-23-CRITICAL-FIXES.md)を保ち、実装レビューのF3/F4/F5を扱う。

- 最終ゴール: 小規模サイトで、安全に解除・復旧できる運用へ進むこと。
- 現マイルストーン: 隔離Linux環境で期限付き遮断と復旧の契約を検証すること。
- 今回の完了条件: XDP自身による期限判定、実執行先への緊急バイパス、期限を延ばさないWAL復元を実装し、異常系・既存回帰・`pi-node` 実機で確認すること。

本番自動遮断の開始判定ではない。期限の保証は以下のローカル契約に限定する。

## 修正と失敗時の契約

### XDPとユーザー空間で同じ期限を使う

`PeerPolicy` は16バイトになり、`expires_at_boot_ns: u64` を持つ。C実装とRust eBPF実装の双方が、`bpf_ktime_get_boot_ns()` で期限を判定する。Linux側は `clock_gettime(CLOCK_BOOTTIME)` を使用する。いずれもサスペンド時間を含む起動後時間で、壁時計のNTP補正には依存しない。ヘルパーの仕様は実験ホストのLinux UAPIヘッダーでも確認した。

期限はローカルで最初に適用準備した時刻から計算する。期限を過ぎたパケットは、デーモンが停止中でもXDPでPASSになる。デーモンの1秒間隔処理は期限切れmap要素の回収と未適用ルールの再試行を担う。回収の失敗が遮断期間を延ばすことはない。`ttl_secs == 0` は従来どおり期限なしであり、この失効保証の対象外。

Linux起動後時計との一致が必要なので、対象はホストと同じ時刻名前空間、boottime offset 0の環境。異なる時刻名前空間のコンテナ運用は検証していない。サスペンドを実機で発生させた試験も未実施。`ConsensusState` の既定の手動時計はシミュレーション用で、通常Linuxデーモンは実時計を明示接続する。

標準ライブラリにはこのLinux時計への直接APIがないため、既存lock内の `libc` をLinux対象の直接依存に追加した。依存バージョンの更新は行っていない。

### 最初の期限を適用前に保存する

WAL v2は、boot ID、`(log index, batch内位置) -> 最初の期限`、バイパスの希望状態を保存する。バッチの全期限をまとめて準備し、checkpoint成功後にmapへ書く。失敗したルールの再試行や部分適用からの再開で期限を計算し直さない。期限切れの記録も残す。

| 復旧条件 | 振る舞い |
| --- | --- |
| 同一OS起動内、元の期限より前 | 元の期限の残時間だけ適用する |
| 元の期限以後、GC実施前または実施後 | BANを復活させない |
| OS再起動でboot IDが変化 | 旧bootの期限付きルールを失効扱いとする |
| commit保存後、期限保存前に停止 | 期限記録のない確定済みTTLルールを失効扱いとする |
| 期限の保存が失敗 | カーネルへそのルールを書かず、永続化エラーを保持する |
| 新しいlog indexとして承認された決定 | 新規のローカル期限を準備できる |

「失効扱い」は可用性を選ぶ判断で、古い攻撃元の遮断を継続する保証を捨てる。継続が必要なら、新しい決定として承認する。WAL内の値1は失効済み記録として予約する。通常の期限は最低でも1秒以上であり、0はカーネルABIの期限なしを意味する。

復元時は確定済みprefixを走査し、IPごとの最終ルールと自ノード向け最終LSM状態だけを執行対象にする。途中のBAN、後で解除されたBANを一時的にカーネルへ再生しない。復元の途中で失敗しても再試行でき、復元対象より後の新規commitを選別から誤って除外しない。

期限記録はログ同様に現時点では圧縮しない。長期稼働時の容量・snapshot I/Oの評価は残る。

### 緊急バイパスを実際のmapへ届ける

`XDP_CONTROL[0]` が1の場合だけXDPの遮断判定を有効にし、それ以外はPASSにする。新規mapのゼロ初期値はPASS。Aya bridgeはmap ABIを検査してからattachする。旧8バイトABIや制御mapのないobjectをそのまま使用できない。

所有者限定Unixソケットを追加した。親ディレクトリは0700、ソケットは0600とし、接続元UIDを確認する。TCPの管理口は開かない。要求の読込み時間と大きさを制限し、XDP backendがない場合や更新失敗時は成功を返さない。

```bash
# 起動ログの「Owner-only XDP bypass control socket」に表示されたパスを使う。
# デーモンと同じUIDで実行。root起動時はsudoを付ける。
veil-node --bypass-local on --control-socket /path/from/startup-log/node.sock

# 原因確認後、残存するルールを再び執行すると判断した場合のみ実行。
veil-node --bypass-local off --control-socket /path/from/startup-log/node.sock
```

有効化は、遅いfsyncを待つ前にカーネルをバイパスする。その後WALを同期し、両方が成功した場合だけ成功応答する。保存に失敗した場合は、その場で通信が戻っていてもエラーとなり、再起動後の維持を保証できない。無効化はWAL保存を先に要求し、保存失敗時に現在のバイパスを解除しない。

保存済みのバイパスは、WAL復元時にルールを書き込む前に適用する。希望状態とmapの同期失敗はdegradedとして扱い、再試行する。metricsのバイパス値は希望状態であり、カーネルの成功確認の代わりにはならない。

バイパスはこのXDPフィルタだけに作用する。LSM隔離、他のファイアウォール、ルーティング、上流障害は解除しない。ソケット処理にはデーモンの応答が必要で、停止中でも操作できる独立管理プロセスではない。TTL失効はその応答に依存しない。

デフォルトのソケットは起動ごとの専用ディレクトリ内に作る。古いソケット・ディレクトリは証拠として残し、自動削除しない。明示パスを再利用する場合も既存ソケットを勝手にunlinkしないため、運用者による確認か新しいパスが必要。

## 互換性と起動条件

- WAL v1には元の期限がない。v2コードは旧WALを自動移行・上書きせず、明確なエラーで起動を拒否する。移行ツールは未実装。**起動を通すためにWALを消して同じ署名鍵を使い直してはいけない**。投票履歴も失われるため、旧データを保全し、履歴を維持した明示的な移行を先に行う必要がある。
- 同梱の `assets/veil_xdp_kern.o` を新ABIで再ビルドした。C source、共通Rust ABI、userspace binary、objectを同じ変更単位で使用する。
- Aya bridgeの対応条件をLinux 5.9以上とした。FDで所有するXDP linkを使用し、プロセス終了でそのattachを解放する。既存XDPプログラムの置換は行わず、二重attachの拒否も実機試験する。
- `--ebpf-obj` を明示した通常デーモンはattach失敗時に終了する。要求した執行を失ったままQUICだけで成功起動しない。object未指定の開発用動作やTUIは、実運用の執行証拠とは区別する。

C objectの再ビルド例（BPF target対応Clangが必要）:

```bash
clang -O2 -g -target bpfel -Wall -Werror \
  -ffile-prefix-map="$PWD"=. -fdebug-compilation-dir=. \
  -c veil-ebpf/src/veil_xdp_kern.c -o assets/veil_xdp_kern.o
```

## 検証

`lease_tests.rs` は、期限直前・直後、GC前後、boot ID変更、期限準備前停止、保存失敗、map適用失敗、過去ルールの再生防止、バイパス復元順序、ソケットでの応答、逆行時計、旧WAL拒否、復元失敗後の新規commitを検証する。従来のQC・二重投票・wire分割の回帰も維持する。

`ttl_linux_tests::linux_live_ttl_bypass_recovery` は明示実行専用で、前提不足をskip成功にしない。既存の隔離fixture `veth-victim` / `ns-veil-rogue` と10.99.0.1 / 10.99.0.66だけを使う。物理NIC・ホストの経路・firewall設定を変更せず、namespace/interfaceも作成・削除しない。

確認した実機ケース:

1. 通常疎通の確認後、期限付きBANでping失敗かつXDP DROPカウンタ増加。
2. 子プロセスへSIGSTOPを送り、`/proc/<pid>/status` で停止を観測。GCループなしでTTL後のping成功かつXDP PASSカウンタ増加。
3. 子プロセスを強制終了し、同じWALから再起動。期限切れBANが復活しない。
4. 新しい期限なしBANでDROPを確認し、所有者限定ソケットでバイパスを有効化してPASS。
5. 再度強制終了・復元し、バイパスが維持されること、明示的なoffでDROP、onでPASSに戻ること。
6. 通常デーモン本体でも同じWALを復元し、実際のCLIでon/offを確認。
7. 全子プロセス終了後にXDP attachが残らず、元の疎通が成功すること。

最初の実機試験はSIGSTOP直後の観測競合で失敗した。停止を観測してから先へ進む待機を試験側へ追加し、合格条件を維持して再実行した。また、並列のローカル試験で時刻だけを使った一時ディレクトリ名が衝突したため、プロセス内連番を追加した。失敗ログも保全する。

最終差分の結果（2026-10-23）:

| 検証 | 結果・範囲 |
| --- | --- |
| macOS / Rust 1.98.1 / workspace test | 46成功・0失敗・1 ignored。ignoredの投票復旧用子fixtureは親試験から明示実行 |
| `pi-node` Linux 7.0.0-38-generic x86_64 / Rust 1.98.1 / workspace test | 45成功・0失敗・3 ignored・3 filtered。TUIと旧ライブ試験を除外し、下記を別途実行 |
| 明示ライブTTL・バイパス・復元試験 | 成功。GC停止中のカーネル期限判定、WAL復元、通常daemon/CLIを含む。TTL段階でPASS=1・DROP=1を観測 |
| 既存Linux XDP送信元ポート・map容量回帰 | rootで別途実行し成功 |
| macOSおよびLinux Clippy | workspace / all-targets / `-D warnings` 成功 |
| C eBPF | Clang 23.1.1でBPF object生成。実カーネルへロード・attach・パケット試験成功 |
| Rust eBPF | 独立検証コピーでnightly + `build-std=core` のBPF target型検査成功。Rust版objectのlink・カーネル検証は未実施 |
| 整形・差分・Markdown | workspace fmt、diff check、Markdown lintを実施 |

Rust eBPFの型検査で、既存の非Copyヘッダーをraw pointerからmoveするビルドエラーも判明した。Ethernet後のIPv4ヘッダーの非整列を考慮して `read_unaligned` へ修正し、C版と同じIHL境界検査を追加した。標準workspaceに含まれないcrateなので、通常のworkspace成功だけでは発見されない問題だった。検証コピーで不足していた依存を取得し、既存nightlyへ `rust-src` を追加した。

証拠はgit管理外の `artifacts/ttl-recovery-6y9lqgsn/` に保全した。`linux-accepted.txt`、`linux-clippy.txt`、`macos-tests.txt`、`macos-clippy.txt`、`rust-ebpf-check-accepted.txt`、ソースhash一覧と差分を含む。実機コピーは `/tmp/veil-ttl-DGhVdPnC`、最終live fixtureは `/tmp/veil-live-lease-491044-29113880792694` に残る。テスト用PKI秘密鍵は証拠archiveへ含めていない。試験終了後の `bpftool net` でXDPが空であることを確認した。

検証したC objectのSHA-256:

```text
d834b8f07972cba228a45da80e42255a6c15376e9ed2f1eb221a9990e7614802
```

明示ライブ試験は `cargo build --workspace --locked` と `cargo test --workspace --locked --no-run` の後、対象テスト実行ファイルにrootで次の引数を渡す。元のfixtureが存在し、XDP未attachでbaseline pingが成功することを前提として検査する。

```text
--ignored --exact ttl_linux_tests::linux_live_ttl_bypass_recovery --nocapture
```

## 残る投入条件

後続修正: 以下は本記録時点の残課題。署名付き共通期限と遅延catch-up対策は[同日の後続実装・検証](2026-10-23-SIGNED-DEADLINE-CATCHUP.md)で追加した。時計・互換性の条件は後続記録を参照。

この修正には、クラスタで署名する `issued_at` / `not_after` は含まれない。従って、まだそのルールを適用したことがないノードへの遅延catch-upでは、過去の相対TTLをそのノードの初回適用時刻から開始する。クラスタ共通の有効期限、時計誤差上限、古い決定の新規適用拒否は次の設計・実装課題である。完全なWAL消失や古いバックアップへの巻戻しにも、このローカル期限記録だけでは対応できない。

BFT進行性と競合prefixの扱い、静的メンバーと公開鍵・アドレスの束縛、通常デーモンの選挙・提案受付、共有CA初期化と鍵配布、管理対象CIDR、実際の検知品質は引き続き投入条件に残る。今回の単一Linuxホスト・隔離vethでの確認を、複数ホストBFT・実サイト・長時間運用・物理電源断の証明へ拡大しない。
