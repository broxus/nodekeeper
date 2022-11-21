## stEVER Node Tools

All-in-one node management tool with stEVER depool support

## How to install

```bash
cargo install --locked --git https://github.com/broxus/stever-node-tools
```

### Node setup

```bash
# Configure node
stever init node

# Optionally (if user isn't root) configure systemd services
sudo stever init systemd

# Deploy contracts (as single or DePool)
stever init contracts
```
