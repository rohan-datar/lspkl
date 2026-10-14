{
  stdenv,
  lib,
  rustPlatform,
  fetchurl,
  runtimeShell,
  coreutils,
  stdlibUrl ? "https://codeload.github.com/apple/pkl/zip/9ed02459c136435a50ea32619c33258c149cf95f",
}:
let
  stdlibArchive = fetchurl {
    name = "pkl-stdlib-source.zip";
    url = stdlibUrl;
    sha256 = "b391f4b92e7808a58fbaed73419fc2071e29b58eaaeac9d3529c82f4f97bcba6";
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
    outputHashes = {
      "tree-sitter-pkl-0.21.0" = "sha256-I6jz9MksI+aiuxMQ3hZ1VyksA+rj3XxRKHfe8gkoO/I=";
    };
  };

  PKL_STDLIB_ARCHIVE = stdlibArchive;

  # These tests create executable shell fixtures at runtime. Only their
  # interpreter changes in the sandbox; their commands and assertions do not.
  postPatch = ''
    substituteInPlace \
      crates/lspkl/tests/json_rpc.rs \
      crates/lspkl/tests/stdio.rs \
      crates/pkl-project/tests/cli.rs \
      crates/pkl-project/tests/project_manager.rs \
      --replace-fail '#!/bin/sh' '#!${runtimeShell}'
    substituteInPlace crates/lspkl/tests/json_rpc.rs crates/lspkl/tests/stdio.rs \
      --replace-fail '/bin/sleep' '${coreutils}/bin/sleep'
    substituteInPlace crates/lspkl/tests/stdio.rs \
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
    install -Dm644 crates/pkl-stdlib/LICENSE "$out/share/doc/lspkl/stdlib/LICENSE"
    install -Dm644 crates/pkl-stdlib/NOTICE "$out/share/doc/lspkl/stdlib/NOTICE"
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
      "x86_64-darwin"
    ];
  };
}
