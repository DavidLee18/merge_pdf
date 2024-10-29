use std::{collections::BTreeMap, path::PathBuf};

use clap::Parser;
use lopdf::{Bookmark, Document, Object, ObjectId};

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long)]
    predir: Option<PathBuf>,

    #[arg(short, long)]
    files: Vec<PathBuf>,

    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> lopdf::Result<()> {
    let args = Args::parse();
    if args.files.len() < 2 {
        println!("ERROR: files must be more than 1");
        return Err(lopdf::Error::Invalid("not enough files".to_string()));
    }
    let predir = args.predir.unwrap_or(PathBuf::from("."));
    let docs = args
        .files
        .into_iter()
        .map(|f| {
            let path = predir.join(f);
            Document::load(path.clone()).map_err(|e| lopdf::Error::Invalid(format!("{:?} is not found", path)))
        })
        .zip(1u32..)
        .map(|(dr, i)| dr.map(|d| (i, d)))
        .collect::<lopdf::Result<Vec<_>>>()?;

    // We use this to keep track of the last Parent per layer depth.
    let mut layer_parent: Vec<Option<u32>> = vec![None; docs.len()];

    // This is the last layer ran.
    let mut last_layer = 0;

    // Define a starting max_id (will be used as start index for object_ids)
    let mut max_id = 1;
    let mut pagenum = 1;
    // Collect all Documents Objects grouped by a map
    let mut documents_pages = BTreeMap::new();
    let mut documents_objects = BTreeMap::new();
    let mut res = Document::new();

    // Let's try to set these to be bigger to avoid multi allocations for faster handling of files.
    // We are just saying each Document it about 1000 objects in size. can be adjusted for better speeds.
    // This can only be used if you use nightly or the #![feature(extend_one)] is stabilized.
    // documents_pages.extend_reserve(documents.len() * 1000);
    // documents_objects.extend_reserve(documents.len() * 1000);

    // Add a Table of Contents
    // We set the object page to (0,0) which means it will point to the first object after it.
    *layer_parent.get_mut(0).ok_or(lopdf::Error::Invalid("layer_parent is empty".to_string()))? = Some(res.add_bookmark(
        Bookmark::new("Table of Contents".to_string(), [0.0, 0.0, 0.0], 0, (0, 0)),
        None,
    ));

    // Can set bookmark formatting and color per report bookmark added.
    // Formating is 1 for italic 2 for bold 3 for bold and italic
    // Color is RGB 0.0..255.0
    for (layer, mut doc) in docs {
        let color = [0.0, 0.0, 0.0];
        let format = 0;
        let mut display = String::new();

        doc.renumber_objects_with(max_id);

        max_id = doc.max_id + 1;

        let mut first_object = None;

        // This is actually better than extend as we use fewer allocations and cloning then.
        for (key, value) in doc.get_pages()
            .into_iter()
            .map(|(_, object_id)| {
                // We use this as the return object for Bookmarking to determine what it points to.
                // We only want to do this for the first page though.
                if first_object.is_none() {
                    first_object = Some(object_id);
                    display = format!("Page {}", pagenum);
                    pagenum += 1;
                }

                (object_id, doc.get_object(object_id).map(|obj| obj.to_owned()))
            }) {
            documents_pages.insert(key, value?);
        }

        documents_objects.extend(doc.objects);

        // Let's shadow our pointer back if nothing then set to (0,0) tto point to the next page
        let object = first_object.unwrap_or((0, 0));

        // This will use the layering to implement children under Parents in the bookmarks
        // Example as we are generating it here.
        // Table of Contents
        // - Page 1
        // -- Page 2
        // -- Page 3
        // --- Page 4

        match layer {
            0 => {
                *layer_parent.get_mut(0).ok_or(lopdf::Error::Invalid("layer_parent is empty".to_string()))? =
                    Some(res.add_bookmark(Bookmark::new(display, color, format, object), None));
                last_layer = 0;
            },
            1 => {
                let parent = *layer_parent.get(0).ok_or(lopdf::Error::Invalid("layer_parent is empty".to_string()))?;
                *layer_parent.get_mut(1).ok_or(lopdf::Error::Invalid("layer_parent[1] is out of index".to_string()))? = Some(res.add_bookmark(
                    Bookmark::new(display, color, format, object),
                    parent,
                ));
                last_layer = 1;
            },
            l if l <= last_layer || l - 1 == last_layer => {
                let parent = *layer_parent.get(l as usize -1).ok_or(lopdf::Error::Invalid("layer_parent is empty".to_string()))?;
                *layer_parent.get_mut(l as usize - 1).ok_or(lopdf::Error::Invalid(format!("layer_parent[{}] is out of index", l)))? = Some(res.add_bookmark(
                    Bookmark::new(display, color, format, object),
                    parent,
                ));
                last_layer = l;
            },
            _ if last_layer > 0 => {
                let parent = *layer_parent.get(last_layer as usize -1).ok_or(lopdf::Error::Invalid(format!("layer_parent[{}] is out of index", last_layer-1)))?;
                *layer_parent.get_mut(last_layer as usize).ok_or(lopdf::Error::Invalid(format!("layer_parent[{}] is out of index", last_layer)))? = Some(res.add_bookmark(
                    Bookmark::new(display, color, format, object),
                    parent,
                ));
            },
            _ => {
                let parent = *layer_parent.get(0).ok_or(lopdf::Error::Invalid(format!("layer_parent[{}] is out of index", 0)))?;
                *layer_parent.get_mut(1).ok_or(lopdf::Error::Invalid(format!("layer_parent[{}] is out of index", 1)))? = Some(res.add_bookmark(
                    Bookmark::new(display, color, format, object),
                    parent,
                ));
                last_layer = 1;
            },
        }
    }

    // Catalog and Pages are mandatory
    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    // Process all objects except "Page" type
    for (object_id, object) in documents_objects.into_iter() {
        // We have to ignore "Page" (as are processed later), "Outlines" and "Outline" objects
        // All other objects should be collected and inserted into the main Document
        match object.type_name().unwrap_or("") {
            "Catalog" => {
                // Collect a first "Catalog" object and use it for the future "Pages"
                catalog_object = Some((
                    if let Some((id, _)) = catalog_object {
                        id
                    } else {
                        object_id
                    },
                    object,
                ));
            }
            "Pages" => {
                // Collect and update a first "Pages" object and use it for the future "Catalog"
                // We have also to merge all dictionaries of the old and the new "Pages" object
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    if let Some((_, ref object)) = pages_object {
                        if let Ok(old_dictionary) = object.as_dict() {
                            dictionary.extend(old_dictionary);
                        }
                    }

                    pages_object = Some((
                        if let Some((id, _)) = pages_object {
                            id
                        } else {
                            object_id
                        },
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            "Page" => {}     // Ignored, processed later and separately
            "Outlines" => {} // Ignored, not supported yet
            "Outline" => {}  // Ignored, not supported yet
            _ => {
                res.objects.insert(object_id, object);
            }
        }
    }

    // If no "Pages" found abort
    if pages_object.is_none() {
        return Err(lopdf::Error::Invalid("Pages root not found.".to_string()));
    }

    // Iter over all "Page" and collect with the parent "Pages" created before
    for (object_id, object) in documents_pages.iter() {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_object.as_ref().unwrap().0);

            res
                .objects
                .insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    // If no "Catalog" found abort
    if catalog_object.is_none() {
        return Err(lopdf::Error::Invalid("Catalog root not found.".to_string()));
    }

    let (catalog_id, catalog_object) = catalog_object.unwrap();
    let (page_id, page_object) = pages_object.unwrap();

    // Build a new "Pages" with updated fields
    if let Ok(dictionary) = page_object.as_dict() {
        let mut dictionary = dictionary.clone();

        // Set new pages count
        dictionary.set("Count", documents_pages.len() as u32);

        // Set new "Kids" list (collected from documents pages) for "Pages"
        dictionary.set(
            "Kids",
            documents_pages
                .into_iter()
                .map(|(object_id, _)| Object::Reference(object_id))
                .collect::<Vec<_>>(),
        );

        res
            .objects
            .insert(page_id, Object::Dictionary(dictionary));
    }

    // Build a new "Catalog" with updated fields
    if let Ok(dictionary) = catalog_object.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", page_id);
        dictionary.set("PageMode", "UseOutlines");
        dictionary.remove(b"Outlines"); // Outlines not supported in merged PDFs

        res
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }

    res.trailer.set("Root", catalog_id);

    // Update the max internal ID as wasn't updated before due to direct objects insertion
    res.max_id = res.objects.len() as u32;

    // Reorder all new Document objects
    res.renumber_objects();

    //Set any Bookmarks to the First child if they are not set to a page
    res.adjust_zero_pages();

    //Set all bookmarks to the PDF Object tree then set the Outlines to the Bookmark content map.
    if let Some(n) = res.build_outline() {
        if let Ok(Object::Dictionary(ref mut dict)) = res.get_object_mut(catalog_id) {
            dict.set("Outlines", Object::Reference(n));
        }
    }

    // Most of the time this does nothing unless there are a lot of streams
    // Can be disabled to speed up the process.
    // document.compress();

    // Save the merged PDF
    // Store file in current working directory.
    res.save(predir.join("merged.pdf"))?;
    Ok(())
}
