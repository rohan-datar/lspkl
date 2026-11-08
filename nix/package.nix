{
  stdenv,
  lib,
  rustPlatform,
  fetchurl,
  runtimeShell,
  coreutils,
  llvmPackages,
  stdlibUrl ? "https://codeload.github.com/apple/pkl/zip/5ee1f296e3503abd8ac8e9b27d197b2f779ea408",
}:
let
  stdlibArchive = fetchurl {
    name = "pkl-stdlib-source.zip";
    url = stdlibUrl;
    sha256 = "52431e90a999872c53dfe6083d6cded5672f7194730bfee6bf1ab19d0461669e";
  };
in
rustPlatform.buildRustPackage {
  pname = "lspkl";
  version = (lib.importTOML ../crates/lspkl/Cargo.toml).package.version;

  src = lib.cleanSourceWith {
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../crates
        ../LICENSE
        ../NOTICE.txt
      ];
    };
    filter =
      path: type:
      let
        name = builtins.baseNameOf path;
      in
      !(builtins.elem name [
        "target"
        "result"
        ".git"
        ".direnv"
        ".lspkl"
        "build"
        "dist"
      ])
      && !(lib.hasPrefix "result-" name)
      && lib.cleanSourceFilter path type;
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  PKL_STDLIB_ARCHIVE = stdlibArchive;

  # GNU ar does not produce the Mach-O archive index needed by Apple's linker.
  env = lib.optionalAttrs stdenv.hostPlatform.isDarwin {
    AR = "${llvmPackages.bintools}/bin/ar";
    RANLIB = "${llvmPackages.bintools}/bin/ranlib";
  };

  # These tests create executable shell fixtures at runtime. Only their
  # interpreter changes in the sandbox; their commands and assertions do not.
  postPatch = ''
    substituteInPlace \
      crates/lspkl/tests/json_rpc.rs \
      crates/lspkl/tests/stdio.rs \
      crates/lspkl/tests/fallback_cache.rs \
      crates/lspkl/tests/auto_sync.rs \
      crates/lspkl/tests/handlers.rs \
      crates/lspkl/tests/project_reload.rs \
      crates/lspkl/tests/settings_regressions.rs \
      crates/lspkl-project/tests/cli.rs \
      crates/lspkl-project/tests/project_manager.rs \
      --replace-fail '#!/bin/sh' '#!${runtimeShell}'
    substituteInPlace crates/lspkl/tests/json_rpc.rs crates/lspkl/tests/stdio.rs \
      crates/lspkl/tests/fallback_cache.rs crates/lspkl/tests/auto_sync.rs \
      --replace-fail '/bin/sleep' '${coreutils}/bin/sleep'
    substituteInPlace crates/lspkl/tests/stdio.rs crates/lspkl/tests/settings_regressions.rs \
      crates/lspkl/tests/auto_sync.rs \
      --replace-fail '/bin/cat' '${coreutils}/bin/cat'
  '';

  cargoBuildFlags = [
    "--workspace"
    "--bins"
    "--example"
    "smoke"
  ];
  cargoTestFlags = [ "--workspace" ];
  checkFlags = [ "--test-threads=1" ];
  doCheck = true;

  postInstall = ''
    install -Dm644 LICENSE "$out/share/doc/lspkl/LICENSE"
    install -Dm644 NOTICE.txt "$out/share/doc/lspkl/NOTICE.txt"
    install -Dm644 crates/lspkl-stdlib/LICENSE "$out/share/doc/lspkl/stdlib/LICENSE"
    install -Dm644 crates/lspkl-stdlib/NOTICE "$out/share/doc/lspkl/stdlib/NOTICE"
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    target/${stdenv.hostPlatform.rust.rustcTarget}/release/examples/smoke --binary "$out/bin/lspkl"
    runHook postInstallCheck
  '';

  meta = {
    description = "Pkl language server";
    license = lib.licenses.asl20;
    mainProgram = "lspkl";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
}
