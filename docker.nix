{ pkgs ? import <nixpkgs> {}, mcp-v8 }:
pkgs.dockerTools.buildLayeredImage {
  name = "mcp-v8";
  tag = "latest";

  # Contents to include in the image
  contents = [
    mcp-v8
    pkgs.cacert  # CA certificates for SSL
    pkgs.bashInteractive  # Basic shell for debugging
    pkgs.coreutils  # Basic utilities
  ];

  # Configure the image
  config = {
    # Set the entrypoint to the mcp-v8 binary
    Cmd = [
      "${mcp-v8}/bin/mcp-v8"
      "--sse-port"
      "8080"
      "--directory-path"
      "/tmp/mcp-v8-heaps"
    ];

    # Environment variables
    Env = [
      "HEAP_STORAGE_PATH=/tmp/mcp-v8-heaps"
      "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
    ];

    # Expose port
    ExposedPorts = {
      "8080/tcp" = {};
    };

    # Create directories and set up user
    # Note: dockerTools doesn't support USER directive the same way,
    # so we run as root by default. For production, you may want to
    # use runAsRoot or extraCommands to set up a non-root user.
  };

  # Run commands to set up the image (runs as root during build)
  extraCommands = ''
    # Create the heaps directory
    mkdir -p tmp/mcp-v8-heaps
    chmod 1777 tmp/mcp-v8-heaps

    # Create a minimal /tmp directory structure
    mkdir -p tmp
    chmod 1777 tmp
  '';

  # Enable content-addressable layers for better caching
  enableFakechroot = true;
  fakeRootCommands = ''
    # Additional setup commands if needed
  '';
}
