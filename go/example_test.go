package mokuji_test

import (
	"context"
	"fmt"
	"log"

	mokuji "github.com/reearth/mokuji/go"
)

func ExampleIndex_Complete() {
	ctx := context.Background()
	index, err := mokuji.OpenFile(ctx, "../conformance/fixtures/names.mokuji")
	if err != nil {
		log.Fatal(err)
	}
	defer func() { _ = index.Close() }()

	// What was typed goes through the index's tokenizer first, so `ＢＬ`
	// finds what `bl` does.
	done, err := index.Complete(ctx, "ＢＬ", 2)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(done.Normalized, done.Payloads, done.FromTable)
	// Output: bl [Black Sea Black Forest] true
}

func ExampleIndex_Search() {
	ctx := context.Background()
	index, err := mokuji.OpenFile(ctx, "../conformance/fixtures/tiny.mokuji")
	if err != nil {
		log.Fatal(err)
	}
	defer func() { _ = index.Close() }()

	found, err := index.Search(ctx, "tokyo chuo", mokuji.Any(), mokuji.Limit(2))
	if err != nil {
		log.Fatal(err)
	}
	for _, r := range found.Results {
		fmt.Printf("%d %.3f %s\n", r.Doc, r.Score, r.Payload)
	}
	fmt.Println(found.Matched, "matched")
	// Output:
	// 1 1.050 35.671,139.772
	// 3 0.693 34.681,135.513
	// 4 matched
}

func ExampleOpenCollection() {
	ctx := context.Background()
	// Only the segments in Japan, and those that state no region.
	collection, err := mokuji.OpenCollection(ctx, "../conformance/fixtures/collection.json",
		&mokuji.CollectionOptions{Countries: []string{"JP"}})
	if err != nil {
		log.Fatal(err)
	}
	defer func() { _ = collection.Close() }()

	done, err := collection.Complete(ctx, "black", 10)
	if err != nil {
		log.Fatal(err)
	}
	for i, hit := range done.Hits {
		fmt.Println(hit.Segment, hit.Doc, done.Payloads[i])
	}
	// Output:
	// 0 1 Black Forest
	// 0 2 Blackpool
	// 1 0 Black Mountain
}
