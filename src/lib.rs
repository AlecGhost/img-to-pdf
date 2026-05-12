use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, content::Content, content::Operation};

pub struct ImageData {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub orientation: u16,
}

pub fn create_pdf(images: &[ImageData]) -> Result<Document> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let catalog_id = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Catalog".to_vec())),
        ("Pages", Object::Reference(pages_id)),
    ]));
    doc.trailer.set("Root", catalog_id);

    let mut page_ids = vec![];

    for img in images {
        let page_id = create_and_add_page(&mut doc, img, pages_id)?;
        page_ids.push(Object::Reference(page_id));
    }

    doc.objects.insert(
        pages_id,
        Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
            ("Kids", Object::Array(page_ids)),
            ("Count", (images.len() as i32).into()),
        ])
        .into(),
    );

    Ok(doc)
}

pub fn insert_pages(doc: &mut Document, images: &[ImageData], start_index: u32) -> Result<()> {
    if images.is_empty() {
        return Ok(());
    }

    let pages = doc.get_pages();
    let page_count = pages.len() as u32;
    let actual_start = start_index.max(1).min(page_count + 1);

    // Find the root Pages object
    let catalog_id = doc.trailer.get(b"Root").and_then(Object::as_reference).context("The PDF file appears to be structurally corrupted. You may need to repair the PDF or recreate it before editing.")?;
    let catalog = doc.get_object(catalog_id).and_then(Object::as_dict)?;
    let pages_root_id = catalog.get(b"Pages").and_then(Object::as_reference).context("The PDF file does not contain any readable pages or its structure is corrupted. You may need to repair the PDF before editing.")?;

    let mut new_page_ids = Vec::new();
    for img in images {
        let page_id = create_and_add_page(doc, img, pages_root_id)?;
        new_page_ids.push(Object::Reference(page_id));
    }

    let pages_dict = doc
        .get_object_mut(pages_root_id)
        .and_then(Object::as_dict_mut)?;
    let count = pages_dict
        .get(b"Count")
        .and_then(Object::as_i64)
        .unwrap_or(0);

    if let Ok(Object::Array(kids)) = pages_dict.get_mut(b"Kids") {
        // Find correct insertion index in the kids array.
        // For simplicity, we just use the actual_start index minus 1 (clamped to kids.len()).
        // This is correct for simple, flat PDF page trees.
        let insert_idx = (actual_start as usize - 1).min(kids.len());
        for (i, new_page) in new_page_ids.into_iter().enumerate() {
            kids.insert(insert_idx + i, new_page);
        }
    }

    pages_dict.set("Count", count + images.len() as i64);

    Ok(())
}

fn create_and_add_page(
    doc: &mut Document,
    img: &ImageData,
    parent_pages_id: ObjectId,
) -> Result<ObjectId> {
    let xobject = lopdf::xobject::image_from(img.data.clone()).context("Failed to embed the image into the PDF. The image data might be invalid or in an unsupported format.")?;
    let xobject_id = doc.add_object(xobject);

    let img_name = format!("Im{}", xobject_id.0);

    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    img.width.into(),
                    0.into(),
                    0.into(),
                    img.height.into(),
                    0.into(),
                    0.into(),
                ],
            ),
            Operation::new("Do", vec![Object::Name(img_name.as_bytes().to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };

    let content_id = doc.add_object(Stream::new(Dictionary::new(), content.encode().unwrap()));

    let mut resources = Dictionary::new();
    let mut xobjects = Dictionary::new();
    xobjects.set(img_name.as_bytes().to_vec(), Object::Reference(xobject_id));
    resources.set("XObject", Object::Dictionary(xobjects));

    resources.set(
        "ProcSet",
        Object::Array(vec![
            Object::Name(b"PDF".to_vec()),
            Object::Name(b"Text".to_vec()),
            Object::Name(b"ImageB".to_vec()),
            Object::Name(b"ImageC".to_vec()),
            Object::Name(b"ImageI".to_vec()),
        ]),
    );

    let rotate = match img.orientation {
        3 => 180,
        6 => 90,
        8 => 270,
        _ => 0,
    };

    let mut page_dict_vec = vec![
        ("Type", Object::Name(b"Page".to_vec())),
        ("Parent", Object::Reference(parent_pages_id)),
        (
            "MediaBox",
            vec![0.into(), 0.into(), img.width.into(), img.height.into()].into(),
        ),
        ("Contents", Object::Reference(content_id)),
        ("Resources", Object::Dictionary(resources)),
    ];

    if rotate != 0 {
        page_dict_vec.push(("Rotate", rotate.into()));
    }

    let page_dict = Dictionary::from_iter(page_dict_vec);

    Ok(doc.add_object(page_dict))
}

pub fn remove_page(doc: &mut Document, page_number: u32) -> Result<()> {
    let pages = doc.get_pages();
    if !pages.contains_key(&page_number) {
        anyhow::bail!(
            "Cannot remove page {}. This page does not exist in the document.",
            page_number
        );
    }
    if pages.len() <= 1 {
        anyhow::bail!(
            "Cannot remove page {}. A PDF document must have at least one page.",
            page_number
        );
    }
    doc.delete_pages(&[page_number]);
    Ok(())
}

pub fn swap_pages(doc: &mut Document, page1: u32, page2: u32) -> Result<()> {
    if page1 == page2 {
        return Ok(());
    }

    let pages = doc.get_pages();
    let pid1 = pages
        .get(&page1)
        .copied()
        .with_context(|| format!("Page index {} is invalid or out of bounds.", page1))?;
    let pid2 = pages
        .get(&page2)
        .copied()
        .with_context(|| format!("Page index {} is invalid or out of bounds.", page2))?;

    fn replace_kid(
        doc: &mut Document,
        parent_id: ObjectId,
        old_kid: ObjectId,
        new_kid: ObjectId,
    ) -> Result<()> {
        let parent = doc
            .get_object_mut(parent_id)
            .and_then(Object::as_dict_mut)?;
        if let Ok(kids) = parent.get_mut(b"Kids")
            && let Object::Array(kids_arr) = kids {
                for kid in kids_arr.iter_mut() {
                    if let Object::Reference(ref_id) = kid
                        && *ref_id == old_kid {
                            *kid = Object::Reference(new_kid);
                            return Ok(());
                        }
                }
            }
        Ok(())
    }

    let parent1 = match doc.get_object(pid1).and_then(Object::as_dict) {
        Ok(d) => match d.get(b"Parent") {
            Ok(Object::Reference(p)) => *p,
            _ => anyhow::bail!(
                "The first page specified exists but its structure within the PDF is broken. Please try repairing the PDF."
            ),
        },
        _ => anyhow::bail!(
            "The first page specified cannot be read because its data is corrupted. Please try repairing the PDF."
        ),
    };

    let parent2 = match doc.get_object(pid2).and_then(Object::as_dict) {
        Ok(d) => match d.get(b"Parent") {
            Ok(Object::Reference(p)) => *p,
            _ => anyhow::bail!(
                "The second page specified exists but its structure within the PDF is broken. Please try repairing the PDF."
            ),
        },
        _ => anyhow::bail!(
            "The second page specified cannot be read because its data is corrupted. Please try repairing the PDF."
        ),
    };

    if parent1 == parent2 {
        let parent = doc.get_object_mut(parent1).and_then(Object::as_dict_mut)?;
        if let Ok(kids) = parent.get_mut(b"Kids")
            && let Object::Array(kids_arr) = kids {
                let mut idx1 = None;
                let mut idx2 = None;
                for (i, kid) in kids_arr.iter().enumerate() {
                    if let Object::Reference(ref_id) = kid {
                        if *ref_id == pid1 {
                            idx1 = Some(i);
                        }
                        if *ref_id == pid2 {
                            idx2 = Some(i);
                        }
                    }
                }
                if let (Some(i1), Some(i2)) = (idx1, idx2) {
                    kids_arr.swap(i1, i2);
                }
            }
    } else {
        replace_kid(doc, parent1, pid1, pid2)?;
        replace_kid(doc, parent2, pid2, pid1)?;

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pid1) {
            d.set("Parent", Object::Reference(parent2));
        }
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pid2) {
            d.set("Parent", Object::Reference(parent1));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_dummy_image() -> ImageData {
        let b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        ImageData {
            data: STANDARD.decode(b64).unwrap(),
            width: 1,
            height: 1,
            orientation: 1,
        }
    }

    #[test]
    fn test_swap_pages() {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let catalog_id = doc.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference(pages_id)),
        ]));
        doc.trailer.set("Root", catalog_id);

        let mut page_ids = vec![];
        for _ in 0..3 {
            let page_dict = Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Page".to_vec())),
                ("Parent", Object::Reference(pages_id)),
            ]);
            page_ids.push(Object::Reference(doc.add_object(page_dict)));
        }

        doc.objects.insert(
            pages_id,
            Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Pages".to_vec())),
                ("Kids", Object::Array(page_ids.clone())),
                ("Count", 3.into()),
            ])
            .into(),
        );

        // Ensure get_pages works
        assert_eq!(doc.get_pages().len(), 3);

        // Remove page 2
        crate::remove_page(&mut doc, 2).unwrap();
        assert_eq!(doc.get_pages().len(), 2);

        // Swap page 1 and 2
        crate::swap_pages(&mut doc, 1, 2).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }

    #[test]
    fn test_cli_sequence() {
        let dummy_img = create_dummy_image();

        // 1. Create
        let mut doc = crate::create_pdf(&[dummy_img]).unwrap();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();

        // 2. Insert at index 1
        let dummy2 = create_dummy_image();
        let mut doc = Document::load_mem(&out).unwrap();
        crate::insert_pages(&mut doc, &[dummy2], 1).unwrap();
        let mut out2 = Vec::new();
        doc.save_to(&mut out2).unwrap();

        // 3. Insert at index 2
        let dummy3 = create_dummy_image();
        let mut doc = Document::load_mem(&out2).unwrap();
        crate::insert_pages(&mut doc, &[dummy3], 2).unwrap();
        let mut out3 = Vec::new();
        doc.save_to(&mut out3).unwrap();

        // 4. Remove page 2
        let mut doc = Document::load_mem(&out3).unwrap();
        crate::remove_page(&mut doc, 2).unwrap();
        let mut out4 = Vec::new();
        doc.save_to(&mut out4).unwrap();

        // 5. Swap
        let mut doc = Document::load_mem(&out4).unwrap();
        crate::swap_pages(&mut doc, 1, 2).unwrap();
    }
}
