# stockroom

Inventory service for the warehouse floor: an in-memory store, a read-through
cache, a small JSON-RPC endpoint for the handheld scanners, and the `stockctl`
command.

## Go API

```go
s := store.New(seed.Demo()...)
item, err := s.GetItem(ctx, "BLT-M8-40")
if errors.Is(err, store.ItemNotFoundErr) {
    // unknown SKU
}
```

`cache.New(s, time.Minute)` wraps anything with a `GetItem` method and also
remembers misses (`store.ItemNotFoundErr`) for the TTL.

`report.LowStock` calls `GetItem` for each SKU and returns the items at or below
a threshold, skipping SKUs that do not exist.

## Wire protocol

The scanners `POST /rpc` with a JSON body:

```json
{"method": "Inventory.GetItem", "params": {"sku": "BLT-M8-40"}}
{"method": "Inventory.ListItems", "params": {}}
{"method": "Inventory.Count", "params": {}}
```

Method names are a contract with scanner firmware already in the field; they
never change. An unknown SKU answers `{"error": {"code": 404, "message": "item not found"}}`.

## Development

```sh
go build ./... && go vet ./... && go test ./...
```
