// The page-level scope switcher of the Skills and MCP pages (SPEC 13.2, 13.7): the user scope
// or one of the projects this machine knows.

import { useQuery } from "@tanstack/react-query";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { unwrap, useTransport } from "@/ipc/transport";

/** The value of the user scope; any other value is a project id. */
export const USER_SCOPE = "user";

export type ScopeSelectProps = {
  value: string;
  ariaLabel: string;
  userLabel: string;
  projectLabel: (name: string) => string;
  onChange: (value: string) => void;
};

export function ScopeSelect({ value, ariaLabel, userLabel, projectLabel, onChange }: ScopeSelectProps) {
  const { commands } = useTransport();
  const projects = useQuery({
    queryKey: storeQueryKeys.projects,
    queryFn: async () => unwrap(await commands.listProjects()),
  });
  const options: AppSelectOption<string>[] = [
    { value: USER_SCOPE, label: userLabel },
    ...(projects.data ?? []).map((row) => ({ value: row.id, label: projectLabel(row.name) })),
  ];
  const selectedLabel = options.find((option) => option.value === value)?.label ?? userLabel;
  return (
    <AppSelect
      className="skills-scope-select"
      value={value}
      options={options}
      ariaLabel={ariaLabel}
      selectedLabel={selectedLabel}
      onChange={onChange}
    />
  );
}
