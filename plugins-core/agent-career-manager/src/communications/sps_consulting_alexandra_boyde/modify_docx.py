from docx import Document
import sys

def modify_docx(file_path):
    doc = Document(file_path)
    
    # Trackers to ensure we only modify what we intend
    context_opt_updated = False
    semantic_agent_updated = False
    
    for table in doc.tables:
        for i, row in enumerate(table.rows):
            # The criteria is usually in the first cell of the row
            criteria_text = row.cells[0].text
            
            if "Erfahrung in Kenntnissen in Context optimization" in criteria_text:
                # The checkbox is likely in the second cell
                # We can't easily tick a visual form checkbox, but we can replace the text representation
                if len(row.cells) > 1:
                    row.cells[1].text = row.cells[1].text.replace("☐", "⌧")
                # The explanation is in the third cell
                if len(row.cells) > 2:
                    text = row.cells[2].text
                    text = text.replace("Erfahrung: +2 Jahre", "Erfahrung: +4 Jahre")
                    if "02/2019 – 07/2024: ChainGO Tech" not in text:
                         text = text.replace("01/2024 – laufend: TrayLinx", "01/2024 – laufend: TrayLinx\n02/2019 – 07/2024: ChainGO Tech")
                    row.cells[2].text = text
                context_opt_updated = True
                
            if "Erfahrung in Kenntnissen in Semantic Agent Optimization" in criteria_text:
                if len(row.cells) > 1:
                    row.cells[1].text = row.cells[1].text.replace("☐", "⌧")
                if len(row.cells) > 2:
                    text = row.cells[2].text
                    text = text.replace("Erfahrung: +2 Jahre", "Erfahrung: +4 Jahre")
                    if "02/2019 – 07/2024: ChainGO Tech" not in text:
                         text = text.replace("01/2024 – laufend: TrayLinx", "01/2024 – laufend: TrayLinx\n02/2019 – 07/2024: ChainGO Tech")
                    row.cells[2].text = text
                semantic_agent_updated = True

    output_path = file_path.replace(".docx", "_updated.docx")
    doc.save(output_path)
    
    print(f"Modifications complete.")
    print(f"Context Optimization Updated: {context_opt_updated}")
    print(f"Semantic Agent Optimization Updated: {semantic_agent_updated}")
    print(f"Saved to: {output_path}")

if __name__ == "__main__":
    import os
    import argparse
    parser = argparse.ArgumentParser(description="Modify DOCX CV profile")
    HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
    default_path = os.path.join(
        HARVEY_HOME, "career", "communications",
        "sps_consulting_alexandra_boyde",
        "CV_Sebastian_Schkudlara_SPS_Consulting_2026-03-11.docx"
    )
    parser.add_argument("--file", default=default_path, help="Path to the DOCX file")
    args = parser.parse_args()
    modify_docx(args.file)
