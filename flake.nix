{
  description = "cli development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    naersk.url = "github:nix-community/naersk";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    naersk,
  }: let
    overlays = [
      (import rust-overlay)
      (self: super: {
        rustToolchain = super.rust-bin.stable.latest.default.override {
          targets = ["wasm32-unknown-unknown"];
          extensions = ["rustfmt" "llvm-tools-preview" "rust-src"];
        };

        # stand-alone nightly formatter so we get the fancy unstable flags
        nightlyRustfmt = super.rust-bin.selectLatestNightlyWith (toolchain:
          toolchain.default.override {
            extensions = ["rustfmt"]; # just the formatter
          });
      })
    ];

    allSystems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];

    forAllSystems = f:
      nixpkgs.lib.genAttrs allSystems (system:
        f {
          pkgs = import nixpkgs {
            inherit overlays system;
          };
          system = system;
        });
  in {
    devShells = forAllSystems ({pkgs, ...}: {
      default = pkgs.mkShell {
        packages =
          (with pkgs; [
            rustToolchain
            nightlyRustfmt
            cargo-sort
            toml-cli
            openssl
            postgresql
            pkg-config
          ])
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs; [
            ]);

        RUSTFMT = "${pkgs.nightlyRustfmt}/bin/rustfmt";
      };
    });

    packages = forAllSystems ({
      pkgs,
      system,
    }: let
      # naersk still builds crate archive URLs through the crates.io API endpoint,
      # which can reject CI fetches. Use the canonical static archive CDN instead.
      fetchCrateFromStaticCdn = args: let
        crateMatch = builtins.match ".*/api/v1/crates/([^/]+)/([^/]+)/download" args.url;
      in
        pkgs.fetchurl (
          args
          // pkgs.lib.optionalAttrs (crateMatch != null) {
            url = let
              name = builtins.elemAt crateMatch 0;
              version = builtins.elemAt crateMatch 1;
            in "https://static.crates.io/crates/${name}/${name}-${version}.crate";
          }
        );
      naersk-lib = pkgs.callPackage naersk {
        cargo = pkgs.rustToolchain;
        fetchurl = fetchCrateFromStaticCdn;
        rustc = pkgs.rustToolchain;
      };
      buildCli = {
        cargoPackage,
        pname,
      }:
        naersk-lib.buildPackage {
          inherit pname;
          version = "0.1.0";
          release = true;
          src = ./.;

          cargoBuildOptions = options: options ++ ["-p" cargoPackage];
          cargoTestOptions = options: options ++ ["-p" cargoPackage];

          buildInputs = [pkgs.openssl pkgs.pkg-config];
        };
    in {
      switchboard = buildCli {
        cargoPackage = "switchboard-cli";
        pname = "switchboard";
      };
      mychart = buildCli {
        cargoPackage = "mychart-cli";
        pname = "mychart";
      };
      mindbody = buildCli {
        cargoPackage = "mindbody-cli";
        pname = "mindbody";
      };
      momence = buildCli {
        cargoPackage = "momence-cli";
        pname = "momence";
      };
      plaid = buildCli {
        cargoPackage = "plaid-cli";
        pname = "plaid";
      };
      schwab = buildCli {
        cargoPackage = "schwab-cli";
        pname = "schwab";
      };
      default = self.packages.${system}.switchboard;
    });
  };
}
