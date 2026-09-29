import React from "react";
import { ToggleSwitch, ToggleSwitchProps } from "./ToggleSwitch";

export type SwitchToggleProps = ToggleSwitchProps;

/**
 * SwitchToggle — standardized alias to ToggleSwitch for backward compatibility.
 */
export const SwitchToggle: React.FC<SwitchToggleProps> = (props) => {
  return <ToggleSwitch {...props} />;
};
