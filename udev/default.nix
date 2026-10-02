{ pkgs, ... }:
{
  services.udev.packages = [
    (pkgs.writeTextDir "lib/udev/rules.d/70-owon-vds1022.rules"
      (builtins.readFile ./70-owon-vds1022.rules))
  ];
}
