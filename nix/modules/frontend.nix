{ inputs, ... }:
{
  perSystem = { pkgs, ... }: {
    packages.frontend = pkgs.buildNpmPackage {
      pname = "train-stations-map-frontend";
      version = "0.1.0";
      src = inputs.self + "/frontend";
      npmDepsHash = "sha256-AU2QJeEVqxUi5GhAXTYgZmFCo3Wcfjtyij4oZ7P7RZ4=";
      installPhase = ''
        runHook preInstall
        cp -r dist $out
        runHook postInstall
      '';
    };
  };
}
