import onnx
import os
import sys

# This script is designed to be self-contained and robust.
# It manually defines the paths to avoid issues with the config file's state.
MODEL_DIR = os.path.join(os.path.dirname(__file__), "models")
# We always want to fix the ORIGINAL model.
INPUT_FILENAME = "version-RFB-320.onnx"
OUTPUT_FILENAME = INPUT_FILENAME.replace('.onnx', '-fixed.onnx')

def fix_model(model_path, output_path):
    """
    Fixes an ONNX model by removing initializers from the graph inputs
    and setting a compatible IR version.
    """
    try:
        model = onnx.load(model_path)
        
        # --- Fix 1: Remove initializers from graph inputs ---
        initializer_names = {initializer.name for initializer in model.graph.initializer}
        new_inputs = [inp for inp in model.graph.input if inp.name not in initializer_names]
        
        graph_changed = len(new_inputs) != len(model.graph.input)

        new_graph = onnx.helper.make_graph(
            model.graph.node,
            model.graph.name,
            new_inputs,
            model.graph.output,
            model.graph.initializer
        )
        
        # --- Fix 2: Set a compatible IR and Opset version ---
        new_model = onnx.helper.make_model(new_graph, producer_name='gemini-onnx-fixer', opset_imports=model.opset_import)
        
        # Force the IR version to be compatible with older onnxruntime versions
        if new_model.ir_version > 11:
            print(f"Original IR version ({new_model.ir_version}) is too high. Downgrading to 11.")
            new_model.ir_version = 11
            ir_changed = True
        else:
            ir_changed = False

        if not graph_changed and not ir_changed:
            print(f"Model '{model_path}' does not appear to need fixing.")
            return True # Nothing to do

        # Verify the corrected model
        onnx.checker.check_model(new_model)
        
        # Save the fixed model
        onnx.save(new_model, output_path)
        print(f"\nSuccessfully created fixed model at: '{output_path}'")
        return True

    except FileNotFoundError:
        print(f"\nERROR: The original model file was not found at '{model_path}'")
        print("Please ensure 'version-RFB-320.onnx' exists in the 'models' directory.")
        return False
    except Exception as e:
        print(f"\nAn unexpected error occurred: {e}")
        return False

if __name__ == '__main__':
    input_path = os.path.join(MODEL_DIR, INPUT_FILENAME)
    output_path = os.path.join(MODEL_DIR, OUTPUT_FILENAME)
    
    print("--- ONNX Model Fixer ---")
    print(f"Input:  {input_path}")
    print(f"Output: {output_path}")
    print("------------------------")
    
    if fix_model(input_path, output_path):
        print("\nAll steps completed. Your server should now be able to start without errors.")
    else:
        print("\nScript failed. Please review the error message above.")
        sys.exit(1)
