import { render } from "solid-js/web";
import "../styles.css";
import { WorkbenchApp } from "./WorkbenchApp";

const root = document.getElementById("root");
if (root) {
  render(() => <WorkbenchApp />, root);
}
