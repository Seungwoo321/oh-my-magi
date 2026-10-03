import type { ButtonHTMLAttributes, ReactNode } from "react";

export function Button({ children, tone = "secondary", disabled = false, type = "button", className, ...attributes }: ButtonHTMLAttributes<HTMLButtonElement> & {
  children: ReactNode;
  tone?: "primary" | "secondary" | "danger";
}) {
  return <button {...attributes} className={`button button-${tone}${className ? ` ${className}` : ""}`} type={type} disabled={disabled}>{children}</button>;
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
