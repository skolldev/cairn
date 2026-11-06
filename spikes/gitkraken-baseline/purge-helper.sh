#!/bin/zsh
# Runs in your terminal so sudo stays yours. Purges the file cache whenever cold.trigger appears.
cd "$(dirname "$0")"; rm -f cold.trigger cold.done
sudo -v || exit 1
echo "Ready. Leave this running; Ctrl-C when the run ends."
while true; do
  sudo -n -v
  if [[ -f cold.trigger ]]; then rm -f cold.trigger; sudo -n purge; date +%T > cold.done; echo "purged $(date +%T)"; fi
  sleep 0.3
done
