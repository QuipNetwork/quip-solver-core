# Test data

The `python-*` files were written by the Python oracle encoder. Each test that reads them names the command.

The two `zeros-*` files are gzip files that hold only zero bytes. The test `oversize_payload_is_rejected` in `topology_blob.rs` reads them. The first file holds one byte more than the payload limit of 64 MiB. The second file holds exactly 64 MiB.

Run these commands from this directory to build them again:

```sh
head -c 67108865 /dev/zero | gzip -9n > zeros-64mib-plus-1.bin.gz
head -c 67108864 /dev/zero | gzip -9n > zeros-64mib.bin.gz
```
