# Install: Nix

If you use Nix, the repo exposes a flake package and a NixOS module.

## Run directly

```bash
nix run github:r33drichards/mcp-js#default -- --help
```

## Install into your profile

```bash
nix profile install github:r33drichards/mcp-js#default
server --help
```

The flake package installs the `server` binary, which is the `mcp-v8` server
executable.

## Use the NixOS module with flakes

The repo also exports `nixosModules.mcp-js`. A minimal flake-based NixOS
configuration looks like this:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    mcp-js.url = "github:r33drichards/mcp-js";
  };

  outputs = { nixpkgs, mcp-js, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        mcp-js.nixosModules.mcp-js
        ({ pkgs, ... }: {
          services.mcp-js = {
            enable = true;
            package = mcp-js.packages.${pkgs.system}.default;
            nodeId = "node1";
            httpPort = 3000;
            dataDir = "/var/lib/mcp-js";
          };
        })
      ];
    };
  };
}
```

That module starts the packaged server as a systemd service and wires the basic
HTTP transport for you.
