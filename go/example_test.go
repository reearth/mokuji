package mokuji_test

import (
	"context"
	"crypto/sha256"
	"fmt"
	"log"
	"maps"
	"slices"

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

func ExampleBuilder() {
	// Places, one document each: the text to index, and the payload a match
	// returns.
	places := [][2]string{
		{"tokyo chiyoda marunouchi", "35.681,139.767"},
		{"tokyo chuo ginza", "35.671,139.772"},
		{"osaka kita umeda", "34.702,135.496"},
	}

	options := mokuji.DefaultWriteOptions()
	builder, err := mokuji.NewBuilder(options)
	if err != nil {
		log.Fatal(err)
	}
	// Documents first, in ID order; then postings, terms in byte order and
	// each term's documents ascending. Sorting is the caller's.
	docs := map[string][]int{}
	for _, place := range places {
		doc, err := builder.AddDocument([]byte(place[1]))
		if err != nil {
			log.Fatal(err)
		}
		terms, _ := mokuji.Split("space-split", place[0])
		for _, term := range terms {
			docs[term] = append(docs[term], doc)
		}
	}
	terms := slices.Sorted(maps.Keys(docs))
	for _, term := range terms {
		for _, doc := range docs[term] {
			if err := builder.AddPosting([]byte(term), doc, 1); err != nil {
				log.Fatal(err)
			}
		}
	}

	// What the index was derived from, by content hash, and how, so that it
	// can be rebuilt and checked.
	var source []byte
	for _, place := range places {
		source = fmt.Appendf(source, "%s\t%s\n", place[0], place[1])
	}
	meta, err := (&mokuji.Metadata{
		Generation:       1,
		Sources:          []mokuji.Provenance{{ContentHash: fmt.Sprintf("sha2-256:%x", sha256.Sum256(source)), URI: "places.tsv"}},
		GeneratorName:    mokuji.GeneratorName,
		GeneratorVersion: mokuji.Version,
		GeneratorParams:  options.Params(),
		TokenizerID:      "space-split",
	}).Encode()
	if err != nil {
		log.Fatal(err)
	}
	file, err := builder.Finish(meta)
	if err != nil {
		log.Fatal(err)
	}

	index, err := mokuji.Open(context.Background(), mokuji.BytesSource(file))
	if err != nil {
		log.Fatal(err)
	}
	found, err := index.Search(context.Background(), "tokyo chuo")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(found.Results[0].Payload)
	// Output: 35.671,139.772
}
