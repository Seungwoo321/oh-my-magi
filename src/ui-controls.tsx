import type { ReactNode } from "react";

export function Button({ children, onClick, tone = "secondary", disabled = false, type = "button", title }: {
  children: ReactNode;
  onClick?: () => void;
  tone?: "primary" | "secondary" | "danger";
  disabled?: boolean;
  type?: "button" | "submit";
  title?: string;
}) {
  return <button className={`button button-${tone}`} type={type} onClick={onClick} disabled={disabled} title={title}>{children}</button>;
}

export function Panel({ title, kicker, children, className = "" }: { title: string; kicker?: string; children: ReactNode; className?: string }) {
  return (
    <section className={`panel ${className}`}>
      {kicker && <p className="panel-kicker">{kicker}</p>}
      <h3>{title}</h3>
      <div className="panel-content">{children}</div>
    </section>
  );
}
