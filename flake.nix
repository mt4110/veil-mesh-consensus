{
  description = "veil-mesh-consensus: 2-node microvm cluster for eBPF (Aya) + QUIC mesh testing";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    microvm = {
      url = "github:astro/microvm.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, microvm, rust-overlay }:
    let
      vmSystem = "aarch64-linux";
      darwinSystem = "aarch64-darwin";

      overlays = [ (import rust-overlay) ];
      pkgsLinux = import nixpkgs { system = vmSystem; inherit overlays; };
      pkgsDarwin = import nixpkgs { system = darwinSystem; inherit overlays; };

      # 【Single Source of Truth】 標準でCONFIG_DEBUG_INFO_BTF=yが有効なカーネル（再ビルド不要）
      sharedKernelPackages = pkgsLinux.linuxPackages_latest;
      kernel = sharedKernelPackages.kernel;

      # 共通VMモジュール生成関数
      mkNode = { hostName, ipAddress, mac0, mac1 }: nixpkgs.lib.nixosSystem {
        system = vmSystem;
        modules = [
          microvm.nixosModules.microvm
          ({ pkgs, ... }: {
            networking.hostName = hostName;

            # カーネルとeBPF/LSM設定
            boot.kernelPackages = sharedKernelPackages;
            boot.kernelParams = [ "lsm=lockdown,yama,loadpin,safesetid,integrity,bpf" ];

            # ネットワーク・ファイアウォール
            networking.useNetworkd = true;
            networking.firewall.enable = true;
            networking.firewall.allowedUDPPorts = [ 5000 ];
            networking.firewall.allowedICMPTypes = [ "echo-request" ];
            networking.interfaces.eth1.ipv4.addresses = [{
              address = ipAddress;
              prefixLength = 24;
            }];

            # microvm設定 (QEMU + Multicast + ホストワークスペース共有)
            microvm = {
              hypervisor = "qemu";
              vcpu = 2;
              mem = 1024;
              interfaces = [
                { type = "user"; id = "eth0"; mac = mac0; }
                {
                  type = "socket";
                  id = "eth1";
                  mac = mac1;
                  socket = { type = "mcast"; mcast = "230.0.0.1:1234"; };
                }
              ];
              # ホストでビルドしたRustバイナリをVM内の /workspace で即実行可能にする
              shares = [{
                proto = "virtiofs";
                tag = "workspace";
                source = ".";
                mountPoint = "/workspace";
              }];
            };

            # デバッグ・検証ツール
            environment.systemPackages = with pkgs; [
              bpftool
              tcpdump
              iproute2
              ethtool
              vim
            ];

            # 開発用：パスワードなしでrootログイン可能にする
            users.users.root.password = "";
            services.getty.autologinUser = "root";
            system.stateVersion = "24.05";
          })
        ];
      };

      # Linux / Darwin 両対応の devShell ビルダー
      mkDevShell = pkgs: pkgs.mkShell {
        packages = with pkgs; [
          (rust-bin.nightly.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" ];
          })
          bpf-linker
          llvmPackages_17.clang
          llvmPackages_17.libllvm
          pkg-config
          openssl
        ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
          bpftool
          aya-tool
          kernel.dev
        ];

        shellHook = ''
          export KERNEL_HEADERS="${kernel.dev}/lib/modules/${kernel.modDirVersion}/build"
          export VMLINUX_PATH="${kernel.dev}/vmlinux"
          export C_INCLUDE_PATH="$KERNEL_HEADERS/include:$KERNEL_HEADERS/arch/arm64/include"
          echo "veil-mesh-consensus devShell loaded (${pkgs.stdenv.hostPlatform.system})."
          echo "   Kernel Version : ${kernel.modDirVersion}"
          echo "   KERNEL_HEADERS : $KERNEL_HEADERS"
          echo "   VMLINUX_PATH   : $VMLINUX_PATH"
        '';
      };
    in {
      nixosConfigurations = {
        node-alpha = mkNode {
          hostName = "node-alpha";
          ipAddress = "10.0.0.1";
          mac0 = "02:00:00:00:00:01";
          mac1 = "02:00:00:00:00:02";
        };
        node-beta = mkNode {
          hostName = "node-beta";
          ipAddress = "10.0.0.2";
          mac0 = "02:00:00:00:01:01";
          mac1 = "02:00:00:00:01:02";
        };
      };

      # `nix run .#node-alpha` で直接起動できるようにRunnerを公開
      packages.${vmSystem} = {
        node-alpha = self.nixosConfigurations.node-alpha.config.microvm.declaredRunner;
        node-beta  = self.nixosConfigurations.node-beta.config.microvm.declaredRunner;
      };

      devShells.${vmSystem}.default = mkDevShell pkgsLinux;
      devShells.${darwinSystem}.default = mkDevShell pkgsDarwin;
    };
}
