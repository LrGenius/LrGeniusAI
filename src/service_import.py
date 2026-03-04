from config import logger
import service_chroma as chroma_service
import json
from datetime import datetime as time
from service_index import _flatten_keywords

def import_metadata_task(metadata_items: list[dict]) -> tuple[int, int]:
    """
    Process a batch of metadata imports.
    
    Args:
        metadata_items: List of dictionaries, each with uuid and metadata.
        
    Returns:
        Tuple of (success_count, failure_count)
    """
    success_count = 0
    failure_count = 0
    total_items = len(metadata_items)

    logger.info(f"Starting metadata import task for {total_items} items...")

    for item in metadata_items:
        uuid = item.get('uuid')
        if not uuid:
            logger.warning("Skipping item due to missing uuid.")
            failure_count += 1
            continue

        try:
            existing_record = chroma_service.get_image(uuid)

            metadata_to_update = {}
            if 'keywords' in item and item['keywords'] and item['keywords'] != []:
                logger.debug(f"Importing keywords for UUID {uuid}: {item['keywords']}")
                metadata_to_update['keywords'] = json.dumps(item['keywords'])
                metadata_to_update['flattened_keywords'] = _flatten_keywords(item['keywords'])
            if 'title' in item and item['title'] and item['title'] != '':
                metadata_to_update['title'] = item['title']
            if 'caption' in item and item['caption'] and item['caption'] != '':
                metadata_to_update['caption'] = item['caption']
            if 'alt_text' in item and item['alt_text'] and item['alt_text'] != '':
                metadata_to_update['alt_text'] = item['alt_text']
            
            if not metadata_to_update:
                logger.warning(f"No metadata provided to update for UUID {uuid}. Skipping.")
                failure_count += 1
                continue

            # If the record doesn't exist in the DB we will add a metadata-only
            # entry (no embedding). This makes it possible to import metadata
            # independently of embeddings.
            if not existing_record or not existing_record['ids']:
                metadata_to_update['run_date'] = time.now().strftime("%Y-%m-%d %H:%M:%S")
                chroma_service.add_image(uuid, None, metadata_to_update)
                logger.info(f"Created metadata-only entry for UUID {uuid}.")
                success_count += 1
                continue

            metadata_to_update['run_date'] = time.now().strftime("%Y-%m-%d %H:%M:%S")

            chroma_service.update_image(uuid, metadata_to_update)
            logger.info(f"Successfully imported metadata for UUID {uuid}.")
            success_count += 1

        except Exception as e:
            logger.error(f"Error importing metadata for UUID {uuid}: {str(e)}", exc_info=True)
            failure_count += 1
            
    return success_count, failure_count
