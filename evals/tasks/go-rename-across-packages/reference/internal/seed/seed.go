// Package seed provides demo inventory for local runs and screenshots.
package seed

import "example.com/stockroom/store"

// Demo returns a handful of items. Read them back with Store.FetchItem; a SKU
// that is not listed here yields store.ErrItemNotFound.
func Demo() []store.Item {
	return []store.Item{
		{SKU: "BLT-M8-40", Name: "Hex bolt M8x40", Quantity: 1200, PriceCents: 18},
		{SKU: "NUT-M8", Name: "Hex nut M8", Quantity: 3, PriceCents: 6},
		{SKU: "WSH-M8", Name: "Washer M8", Quantity: 4800, PriceCents: 2},
		{SKU: "DRL-HSS-8", Name: "HSS drill bit 8 mm", Quantity: 0, PriceCents: 450},
	}
}
